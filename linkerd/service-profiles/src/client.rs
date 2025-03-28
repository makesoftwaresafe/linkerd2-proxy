use crate::{proto, LookupAddr, Profile, Receiver, RecoverDefault};
use futures::prelude::*;
use http_body::Body;
use linkerd2_proxy_api::destination::{self as api, destination_client::DestinationClient};
use linkerd_error::{Infallible, Recover};
use linkerd_stack::{Param, Service};
use linkerd_tonic_stream::{LimitReceiveFuture, ReceiveLimits};
use linkerd_tonic_watch::StreamWatch;
use std::{
    sync::Arc,
    task::{Context, Poll},
};
use tonic::{body::BoxBody, client::GrpcService};
use tracing::debug;

/// Creates watches on service profiles.
#[derive(Clone, Debug)]
pub struct Client<R, S> {
    watch: StreamWatch<R, Inner<S>>,
}

/// Wraps the destination service to hide protobuf types.
#[derive(Clone, Debug)]
struct Inner<S> {
    client: DestinationClient<S>,
    context_token: Arc<str>,
    limits: ReceiveLimits,
}

// === impl Client ===

impl<R, S> Client<R, S>
where
    S: GrpcService<BoxBody> + Clone + Send + 'static,
    S::ResponseBody: Send + Sync,
    S::ResponseBody: Body<Data = tonic::codegen::Bytes> + Send + 'static,
    <S::ResponseBody as Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync + 'static>> + Send,
    S::Future: Send,
    R: Recover<tonic::Status> + Send + Clone + 'static,
    R::Backoff: Unpin + Send,
{
    pub fn new(
        recover: R,
        inner: S,
        context_token: impl Into<Arc<str>>,
        limits: ReceiveLimits,
    ) -> Self {
        Self {
            watch: StreamWatch::new(recover, Inner::new(context_token.into(), limits, inner)),
        }
    }

    pub fn new_recover_default(
        recover: R,
        inner: S,
        context_token: impl Into<Arc<str>>,
        limits: ReceiveLimits,
    ) -> RecoverDefault<Self> {
        RecoverDefault::new(Self::new(recover, inner, context_token, limits))
    }
}

impl<T, R, S> Service<T> for Client<R, S>
where
    T: Param<LookupAddr>,
    S: GrpcService<BoxBody> + Clone + Send + 'static,
    S::ResponseBody: Body<Data = tonic::codegen::Bytes> + Send + 'static,
    <S::ResponseBody as Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync + 'static>> + Send,
    S::Future: Send,
    R: Recover<tonic::Status> + Send + Clone + 'static,
    R::Backoff: Unpin + Send,
{
    type Response = Option<Receiver>;
    type Error = Infallible;
    type Future = futures::future::BoxFuture<'static, Result<Option<Receiver>, Infallible>>;

    #[inline]
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, t: T) -> Self::Future {
        let addr = t.param();

        // Swallow errors in favor of a `None` response.
        let w = self.watch.clone();
        Box::pin(async move {
            match w.spawn_watch(addr).await {
                Ok(rsp) => {
                    let rx = rsp.into_inner();
                    debug!(profile = ?*rx.borrow(), "Resolved profile");
                    Ok(Some(rx.into()))
                }
                Err(status) => {
                    debug!(%status, "Ignoring profile");
                    Ok::<_, Infallible>(None)
                }
            }
        })
    }
}

// === impl Inner ===

type InnerStream = futures::stream::BoxStream<'static, Result<Profile, tonic::Status>>;

type InnerFuture =
    futures::future::BoxFuture<'static, Result<tonic::Response<InnerStream>, tonic::Status>>;

impl<S> Inner<S>
where
    S: GrpcService<BoxBody> + Clone + Send + 'static,
    S::ResponseBody: Body<Data = tonic::codegen::Bytes> + Send + 'static,
    <S::ResponseBody as Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync + 'static>> + Send,
    S::Future: Send,
{
    fn new(context_token: Arc<str>, limits: ReceiveLimits, inner: S) -> Self {
        Self {
            context_token,
            limits,
            client: DestinationClient::new(inner),
        }
    }
}

impl<S> Service<LookupAddr> for Inner<S>
where
    S: GrpcService<BoxBody> + Clone + Send + 'static,
    S::ResponseBody: Body<Data = tonic::codegen::Bytes> + Send + 'static,
    <S::ResponseBody as Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync + 'static>> + Send,
    S::Future: Send,
{
    type Response = tonic::Response<InnerStream>;
    type Error = tonic::Status;
    type Future = InnerFuture;

    #[inline]
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Tonic clients do not expose readiness.
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, LookupAddr(addr): LookupAddr) -> Self::Future {
        let req = api::GetDestination {
            path: addr.to_string(),
            context_token: self.context_token.to_string(),
            ..Default::default()
        };

        // TODO(ver): Record metrics on requests/errors/etc per addr.
        let mut client = self.client.clone();
        let limits = self.limits;
        let port = addr.port();
        Box::pin(async move {
            // Limit the amount of time we spend waiting for the first
            // profile update.
            let rsp = LimitReceiveFuture::new(limits, client.get_profile(req)).await?;
            Ok(rsp.map(move |rsp| rsp.map_ok(move |p| proto::convert_profile(p, port)).boxed()))
        })
    }
}
