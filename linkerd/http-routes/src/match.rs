mod header;
mod host;
mod path;
mod query_param;

pub(crate) use self::host::HostMatch;
pub use self::{
    header::MatchHeader,
    host::{InvalidHost, MatchHost},
    path::MatchPath,
    query_param::MatchQueryParam,
};

#[derive(Clone, Debug, Default, Hash, PartialEq)]
pub struct MatchRequest {
    pub path: Option<MatchPath>,
    pub headers: Vec<MatchHeader>,
    pub query_params: Vec<MatchQueryParam>,
    pub method: Option<http::Method>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct RequestMatch {
    path_match_length: usize,
    headers: usize,
    query_params: usize,
    method: bool,
}

// === impl MatchRequest ===

impl MatchRequest {
    pub(crate) fn summarize_match<B>(&self, req: &http::Request<B>) -> Option<RequestMatch> {
        let mut summary = RequestMatch::default();

        if let Some(method) = &self.method {
            if req.method() != *method {
                return None;
            }
            summary.method = true;
        }

        if let Some(path) = &self.path {
            summary.path_match_length = path.match_length(req.uri())?;
        }

        if !self.headers.iter().all(|h| h.is_match(req.headers())) {
            return None;
        }
        summary.headers = self.headers.len();

        if !self.query_params.iter().all(|h| h.is_match(req.uri())) {
            return None;
        }
        summary.query_params = self.query_params.len();

        Some(summary)
    }
}

impl Default for RequestMatch {
    fn default() -> Self {
        // Per the gateway spec:
        //
        // > If no matches are specified, the default is a prefix path match on
        // > "/", which has the effect of matching every HTTP request.
        Self {
            path_match_length: 1,
            headers: 0,
            query_params: 0,
            method: false,
        }
    }
}

// === impl RequestMatch ===

impl std::cmp::PartialOrd for RequestMatch {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::cmp::Ord for RequestMatch {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match self.path_match_length.cmp(&other.path_match_length) {
            Ordering::Equal => match self.headers.cmp(&other.headers) {
                Ordering::Equal => match self.query_params.cmp(&other.query_params) {
                    Ordering::Equal => self.method.cmp(&other.method),
                    ord => ord,
                },
                ord => ord,
            },
            ord => ord,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::{HeaderName, HeaderValue};

    // Empty matches apply to all requests.
    #[test]
    fn empty_match() {
        let m = MatchRequest::default();

        let req = http::Request::builder().body(()).unwrap();
        assert_eq!(m.summarize_match(&req), Some(RequestMatch::default()));

        let req = http::Request::builder()
            .method(http::Method::HEAD)
            .body(())
            .unwrap();
        assert_eq!(m.summarize_match(&req), Some(RequestMatch::default()));
    }

    #[test]
    fn method() {
        let m = MatchRequest {
            method: Some(http::Method::GET),
            ..MatchRequest::default()
        };

        let req = http::Request::builder()
            .uri("http://example.com/foo")
            .body(())
            .unwrap();
        assert_eq!(
            m.summarize_match(&req),
            Some(RequestMatch {
                method: true,
                ..Default::default()
            })
        );

        let req = http::Request::builder()
            .method(http::Method::HEAD)
            .uri("https://example.org/")
            .body(())
            .unwrap();
        assert_eq!(m.summarize_match(&req), None);
    }

    #[test]
    fn headers() {
        let m = MatchRequest {
            headers: vec![
                MatchHeader::Exact(
                    HeaderName::from_static("x-foo"),
                    HeaderValue::from_static("bar"),
                ),
                MatchHeader::Regex(HeaderName::from_static("x-baz"), "qu+x".parse().unwrap()),
            ],
            ..MatchRequest::default()
        };

        let req = http::Request::builder()
            .uri("http://example.com/foo")
            .body(())
            .unwrap();
        assert_eq!(m.summarize_match(&req), None);

        let req = http::Request::builder()
            .uri("https://example.org/")
            .header("x-foo", "bar")
            .header("x-baz", "zab") // invalid header value
            .body(())
            .unwrap();
        assert_eq!(m.summarize_match(&req), None);

        // Regex matches apply
        let req = http::Request::builder()
            .uri("https://example.org/")
            .header("x-foo", "bar")
            .header("x-baz", "quuuux")
            .body(())
            .unwrap();
        assert_eq!(
            m.summarize_match(&req),
            Some(RequestMatch {
                headers: 2,
                ..RequestMatch::default()
            })
        );

        // Regex must be anchored.
        let req = http::Request::builder()
            .uri("https://example.org/")
            .header("x-foo", "bar")
            .header("x-baz", "quxa")
            .body(())
            .unwrap();
        assert_eq!(m.summarize_match(&req), None);
    }

    #[test]
    fn path() {
        let m = MatchRequest {
            path: Some(MatchPath::Exact("/foo/bar".to_string())),
            ..MatchRequest::default()
        };

        let req = http::Request::builder()
            .uri("http://example.com/foo")
            .body(())
            .unwrap();
        assert_eq!(m.summarize_match(&req), None);

        let req = http::Request::builder()
            .uri("https://example.org/foo/bar")
            .body(())
            .unwrap();
        assert_eq!(
            m.summarize_match(&req),
            Some(RequestMatch {
                path_match_length: "/foo/bar".len(),
                ..Default::default()
            })
        );
    }

    #[test]
    fn multiple() {
        let m = MatchRequest {
            path: Some(MatchPath::Exact("/foo/bar".to_string())),
            headers: vec![MatchHeader::Exact(
                HeaderName::from_static("x-foo"),
                HeaderValue::from_static("bar"),
            )],
            query_params: vec![MatchQueryParam::Exact("foo".to_string(), "bar".to_string())],
            method: Some(http::Method::GET),
        };

        let req = http::Request::builder()
            .uri("https://example.org/foo/bar?foo=bar")
            .header("x-foo", "bar")
            .body(())
            .unwrap();
        assert_eq!(
            m.summarize_match(&req),
            Some(RequestMatch {
                path_match_length: "/foo/bar".len(),
                headers: 1,
                query_params: 1,
                method: true,
            })
        );

        // One invalid field (method) invalidates the match.
        let req = http::Request::builder()
            .method(http::Method::HEAD)
            .uri("https://example.org/foo/bar?foo=bar")
            .header("x-foo", "bar")
            .body(())
            .unwrap();
        assert_eq!(m.summarize_match(&req), None);
    }
}
