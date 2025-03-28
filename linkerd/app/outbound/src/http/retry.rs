use futures::{future, FutureExt};
use linkerd_app_core::{
    classify,
    http_metrics::retries::Handle,
    metrics::{self, ProfileRouteLabels},
    profiles::{self, http::Route},
    proxy::http::{Body, ClientHandle, EraseResponse},
    svc::{layer, Either, Param},
    Error, Result,
};
use linkerd_http_classify::{Classify, ClassifyEos, ClassifyResponse};
use linkerd_http_retry::{
    peek_trailers::{self, PeekTrailersBody},
    ReplayBody,
};
use linkerd_retry as retry;
use std::sync::Arc;

pub fn layer<N>(
    metrics: metrics::HttpProfileRouteRetry,
) -> impl layer::Layer<N, Service = retry::NewRetry<NewRetryPolicy, N, EraseResponse<()>>> + Clone {
    // Because we wrap the response body type on retries, we must include a
    // `Proxy` middleware for unifying the response body types of the retry
    // and non-retry services.
    retry::NewRetry::layer(NewRetryPolicy::new(metrics), EraseResponse::new(()))
}

#[derive(Clone, Debug)]
pub struct NewRetryPolicy {
    metrics: metrics::HttpProfileRouteRetry,
}

#[derive(Clone, Debug)]
pub struct RetryPolicy {
    metrics: Handle,
    budget: Arc<retry::TpsBudget>,
    response_classes: profiles::http::ResponseClasses,
}

/// Allow buffering requests up to 64 kb
const MAX_BUFFERED_BYTES: usize = 64 * 1024;

// === impl NewRetryPolicy ===

impl NewRetryPolicy {
    pub fn new(metrics: metrics::HttpProfileRouteRetry) -> Self {
        Self { metrics }
    }
}

impl<T> retry::NewPolicy<T> for NewRetryPolicy
where
    T: Param<Route> + Param<ProfileRouteLabels>,
{
    type Policy = RetryPolicy;

    fn new_policy(&self, target: &T) -> Option<Self::Policy> {
        let route: Route = target.param();
        let labels: ProfileRouteLabels = target.param();
        Some(RetryPolicy {
            metrics: self.metrics.get_handle(labels),
            budget: route.retries()?.budget().clone(),
            response_classes: route.response_classes().clone(),
        })
    }
}

// === impl Retry ===

impl<ReqB, RspB>
    retry::Policy<http::Request<ReplayBody<ReqB>>, http::Response<PeekTrailersBody<RspB>>, Error>
    for RetryPolicy
where
    ReqB: Body + Unpin,
    ReqB::Error: Into<Error>,
    RspB: Body + Unpin,
{
    type Future = future::Ready<()>;

    fn retry(
        &mut self,
        req: &mut http::Request<ReplayBody<ReqB>>,
        result: &mut Result<http::Response<PeekTrailersBody<RspB>>, Error>,
    ) -> Option<Self::Future> {
        use retry::Budget as _;

        let retryable = match result {
            Err(_) => false,
            Ok(rsp) => {
                // is the request a failure?
                let is_failure = classify::Request::from(self.response_classes.clone())
                    .classify(req)
                    .start(rsp)
                    .eos(rsp.body().peek_trailers())
                    .is_failure();
                // did the body exceed the maximum length limit?
                let exceeded_max_len = req.body().is_capped();
                let retryable = if let Some(false) = exceeded_max_len {
                    // If the body hasn't exceeded our length limit, we should
                    // retry the request if it's a failure of some sort.
                    is_failure
                } else {
                    // We received a response before the request body was fully
                    // finished streaming. To be safe, we will consider this
                    // as an unretryable request.
                    false
                };
                tracing::trace!(is_failure, ?exceeded_max_len, retryable);
                retryable
            }
        };

        if !retryable {
            self.budget.deposit();
            return None;
        }

        let withdrew = self.budget.withdraw();
        self.metrics.incr_retryable(withdrew);
        if !withdrew {
            return None;
        }

        Some(future::ready(()))
    }

    fn clone_request(
        &mut self,
        req: &http::Request<ReplayBody<ReqB>>,
    ) -> Option<http::Request<ReplayBody<ReqB>>> {
        // Since the body is already wrapped in a ReplayBody, it must not be obviously too large to
        // buffer/clone.
        let mut clone = http::Request::new(req.body().clone());
        *clone.method_mut() = req.method().clone();
        *clone.uri_mut() = req.uri().clone();
        *clone.headers_mut() = req.headers().clone();
        *clone.version_mut() = req.version();

        // The HTTP server sets a ClientHandle with the client's address and a means to close the
        // server-side connection.
        if let Some(client_handle) = req.extensions().get::<ClientHandle>().cloned() {
            clone.extensions_mut().insert(client_handle);
        }

        if let Some(classify) = req.extensions().get::<classify::Response>().cloned() {
            clone.extensions_mut().insert(classify);
        }

        Some(clone)
    }
}

impl<ReqB, RspB> retry::PrepareRetry<http::Request<ReqB>, http::Response<RspB>> for RetryPolicy
where
    ReqB: Body + Unpin,
    ReqB::Error: Into<Error>,
    RspB: Body + Unpin + Send + 'static,
    RspB::Data: Unpin + Send,
    RspB::Error: Unpin + Send,
{
    type RetryRequest = http::Request<ReplayBody<ReqB>>;
    type RetryResponse = http::Response<PeekTrailersBody<RspB>>;
    type ResponseFuture = future::Map<
        peek_trailers::WithPeekTrailersBody<RspB>,
        fn(
            http::Response<PeekTrailersBody<RspB>>,
        ) -> Result<http::Response<PeekTrailersBody<RspB>>>,
    >;

    fn prepare_request(
        self,
        req: http::Request<ReqB>,
    ) -> Either<(Self, Self::RetryRequest), http::Request<ReqB>> {
        let (head, body) = req.into_parts();
        let replay_body = match ReplayBody::try_new(body, MAX_BUFFERED_BYTES) {
            Ok(body) => body,
            Err(body) => {
                tracing::debug!(
                    size = body.size_hint().lower(),
                    "Body is too large to buffer"
                );
                return Either::Right(http::Request::from_parts(head, body));
            }
        };

        // The body may still be too large to be buffered if the body's length was not known.
        // `ReplayBody` handles this gracefully.
        Either::Left((self, http::Request::from_parts(head, replay_body)))
    }

    /// If the response is HTTP/2, return a future that checks for a `TRAILERS`
    /// frame immediately after the first frame of the response.
    fn prepare_response(rsp: http::Response<RspB>) -> Self::ResponseFuture {
        PeekTrailersBody::map_response(rsp).map(Ok)
    }
}
