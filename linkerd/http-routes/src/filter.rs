use http::header::{HeaderName, HeaderValue};

#[derive(Clone, Debug, Hash, PartialEq)]
pub enum Filter {
    ModifyRequestHeader(ModifyRequestHeader),
    RedirectRequest(RedirectRequest),
}

#[derive(Clone, Debug, Hash, PartialEq)]
pub struct ModifyRequestHeader {
    pub add: Vec<(HeaderName, HeaderValue)>,
    pub set: Vec<(HeaderName, HeaderValue)>,
    pub remove: Vec<HeaderName>,
}

#[derive(Clone, Debug, Hash, PartialEq)]
pub struct RedirectRequest {
    pub scheme: Option<http::uri::Scheme>,
    pub authority: http::uri::Authority,
    pub path: Option<PathModifier>,
    pub status_code: http::StatusCode,
}

#[derive(Clone, Debug, Hash, PartialEq)]
pub enum PathModifier {}
