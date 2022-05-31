pub mod filter;
pub mod r#match;

pub use self::{
    filter::Filter,
    r#match::{MatchHost, MatchRequest},
};
use r#match::{HostMatch, RequestMatch};
use std::collections::BTreeMap;

#[cfg(feature = "inbound")]
#[derive(Clone, Debug, Hash, PartialEq)]
pub struct InboundRoutes(pub Vec<InboundRoute>);

#[cfg(feature = "outbound")]
#[derive(Clone, Debug, Hash, PartialEq)]
pub struct OutboundRoutes(pub Vec<OutboundRoute>);

#[cfg(feature = "inbound")]
#[derive(Clone, Debug, Default, Hash, PartialEq)]
pub struct InboundRoute {
    pub hosts: Vec<MatchHost>,
    pub rules: Vec<InboundRule>,
    pub labels: BTreeMap<String, String>,
    // TODO Authorizations (inbound)
}

#[cfg(feature = "inbound")]
#[derive(Clone, Debug, Default, Hash, PartialEq)]
pub struct InboundRule {
    pub matches: Vec<MatchRequest>,
    pub filters: Vec<Filter>,
    pub labels: BTreeMap<String, String>,
}

#[cfg(feature = "outbound")]
#[derive(Clone, Debug, Default, Hash, PartialEq)]
pub struct OutboundRoute {
    pub hosts: Vec<MatchHost>,
    pub rules: Vec<OutboundRule>,
    pub labels: BTreeMap<String, String>,
}

#[cfg(feature = "outbound")]
#[derive(Clone, Debug, Hash, PartialEq)]
pub struct OutboundRule {
    pub matches: Vec<MatchRequest>,
    pub filters: Vec<Filter>,
    pub backends: Vec<Backend>,
    pub labels: BTreeMap<String, String>,
}

#[cfg(feature = "outbound")]
#[derive(Clone, Debug, Hash, PartialEq)]
pub struct Backend {
    pub filters: Vec<Filter>,
    pub coordinate: BackendCoordinate,
}

#[cfg(feature = "outbound")]
#[derive(Clone, Debug, Hash, PartialEq)]
pub struct BackendCoordinate(String);

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct RouteMatch {
    host_match: Option<HostMatch>,
    r#match: RequestMatch,
}

// === impl InboundRoutes ===

#[cfg(feature = "inbound")]
impl InboundRoutes {
    pub fn find_route<B>(&self, req: &http::Request<B>) -> Option<(&InboundRoute, &InboundRule)> {
        self.0
            .iter()
            .filter_map(|rt| rt.find_rule(req).map(|(rl, m)| ((rt, rl), m)))
            // This is roughly equivalent to
            //   max_by(|(_, m0), (_, m)| m0.cmp(m))
            // but we want to ensure that the first match wins.
            .reduce(|(r0, m0), (r, m)| if m0 < m { (r, m) } else { (r0, m0) })
            .map(|(r, _)| r)
    }
}

// === impl InboundRoute ===

#[cfg(feature = "inbound")]
impl InboundRoute {
    pub fn find_rule<B>(&self, req: &http::Request<B>) -> Option<(&InboundRule, RouteMatch)> {
        RouteMatch::find(req, &*self.hosts, self.rules.iter().map(|r| &*r.matches))
            .map(|(idx, rm)| (&self.rules[idx], rm))
    }
}

// === impl OutboundRoutes ===

#[cfg(feature = "outbound")]
impl OutboundRoutes {
    pub fn find_route<B>(&self, req: &http::Request<B>) -> Option<(&OutboundRoute, &OutboundRule)> {
        self.0
            .iter()
            .filter_map(|rt| rt.find_rule(req).map(|(rl, m)| ((rt, rl), m)))
            // This is roughly equivalent to
            //   max_by(|(_, m0), (_, m)| m0.cmp(m))
            // but we want to ensure that the first match wins.
            .reduce(|(r0, m0), (r, m)| if m0 < m { (r, m) } else { (r0, m0) })
            .map(|(r, _)| r)
    }
}

// === impl OutboundRoute ===

#[cfg(feature = "outbound")]
impl OutboundRoute {
    pub fn find_rule<B>(&self, req: &http::Request<B>) -> Option<(&OutboundRule, RouteMatch)> {
        RouteMatch::find(req, &*self.hosts, self.rules.iter().map(|r| &*r.matches))
            .map(|(idx, rm)| (&self.rules[idx], rm))
    }
}

// === impl RouteMatch ===

impl RouteMatch {
    fn find<'r, B>(
        req: &http::Request<B>,
        hosts: &[MatchHost],
        rules: impl Iterator<Item = &'r [MatchRequest]>,
    ) -> Option<(usize, Self)> {
        let host_match = if hosts.is_empty() {
            None
        } else {
            let uri = req.uri();
            let hm = hosts.iter().filter_map(|a| a.summarize_match(uri)).max()?;
            Some(hm)
        };

        rules
            .enumerate()
            .filter_map(|(idx, matches)| {
                // If there are no matches in the list, then the rule has an
                // implicit default match.
                if matches.is_empty() {
                    return Some((idx, RequestMatch::default()));
                }
                matches
                    .iter()
                    .filter_map(|m| m.summarize_match(req))
                    .max()
                    .map(|s| (idx, s))
            })
            // This is roughly equivalent to
            //   max_by(|(_, m0), (_, m)| m0.cmp(m))
            // but we want to ensure that the first match wins.
            .reduce(|(i0, m0), (i, m)| if m0 < m { (i, m) } else { (i0, m0) })
            .map(|(i, s)| {
                let m = Self {
                    host_match,
                    r#match: s,
                };
                (i, m)
            })
    }
}

impl std::cmp::PartialOrd for RouteMatch {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::cmp::Ord for RouteMatch {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match self.host_match.cmp(&other.host_match) {
            Ordering::Equal => self.r#match.cmp(&other.r#match),
            ord => ord,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{r#match::*, *};

    #[test]
    fn inbound_find_route_hostname_precedence() {
        // Given two equivalent routes, choose the explicit hostname match and
        // not the wildcard.
        let rts = InboundRoutes(vec![
            InboundRoute {
                hosts: vec!["*.example.com".parse().unwrap()],
                rules: vec![InboundRule {
                    matches: vec![MatchRequest {
                        path: Some(MatchPath::Exact("/foo".to_string())),
                        ..MatchRequest::default()
                    }],
                    ..InboundRule::default()
                }],
                ..InboundRoute::default()
            },
            InboundRoute {
                hosts: vec!["foo.example.com".parse().unwrap()],
                rules: vec![InboundRule {
                    matches: vec![MatchRequest {
                        path: Some(MatchPath::Exact("/foo".to_string())),
                        ..MatchRequest::default()
                    }],
                    labels: maplit::btreemap! {
                        "expected".to_string() => "".to_string(),
                    },
                    ..InboundRule::default()
                }],
                ..InboundRoute::default()
            },
        ]);

        let req = http::Request::builder()
            .uri("http://foo.example.com/foo")
            .body(())
            .unwrap();
        let (_, rule) = rts.find_route(&req).expect("must match");
        assert!(
            rule.labels.contains_key("expected"),
            "incorrect rule matched"
        );
    }

    #[test]
    fn inbound_find_route_path_length_precedence() {
        // Given two equivalent routes, choose the longer path match.
        let rts = InboundRoutes(vec![
            InboundRoute {
                rules: vec![InboundRule {
                    matches: vec![MatchRequest {
                        path: Some(MatchPath::Prefix("/foo".to_string())),
                        ..MatchRequest::default()
                    }],
                    ..InboundRule::default()
                }],
                ..InboundRoute::default()
            },
            InboundRoute {
                rules: vec![InboundRule {
                    matches: vec![MatchRequest {
                        path: Some(MatchPath::Exact("/foo/bar".to_string())),
                        ..MatchRequest::default()
                    }],
                    labels: maplit::btreemap! {
                        "expected".to_string() => "".to_string(),
                    },
                    ..InboundRule::default()
                }],
                ..InboundRoute::default()
            },
        ]);

        let req = http::Request::builder()
            .uri("http://foo.example.com/foo/bar")
            .body(())
            .unwrap();
        let (_, rule) = rts.find_route(&req).expect("must match");
        assert!(
            rule.labels.contains_key("expected"),
            "incorrect rule matched"
        );
    }

    #[test]
    fn inbound_find_route_header_count_precedence() {
        // Given two routes with header matches, use the one that matches more
        // headers.
        let rts = InboundRoutes(vec![
            InboundRoute {
                rules: vec![InboundRule {
                    matches: vec![MatchRequest {
                        headers: vec![
                            MatchHeader::Exact("x-foo".parse().unwrap(), "bar".parse().unwrap()),
                            MatchHeader::Exact("x-baz".parse().unwrap(), "qux".parse().unwrap()),
                        ],
                        ..MatchRequest::default()
                    }],
                    ..InboundRule::default()
                }],
                ..InboundRoute::default()
            },
            InboundRoute {
                rules: vec![InboundRule {
                    matches: vec![MatchRequest {
                        headers: vec![
                            MatchHeader::Exact("x-foo".parse().unwrap(), "bar".parse().unwrap()),
                            MatchHeader::Exact("x-baz".parse().unwrap(), "qux".parse().unwrap()),
                            MatchHeader::Exact("x-biz".parse().unwrap(), "qyx".parse().unwrap()),
                        ],
                        ..MatchRequest::default()
                    }],
                    labels: maplit::btreemap! {
                        "expected".to_string() => "".to_string(),
                    },
                    ..InboundRule::default()
                }],
                ..InboundRoute::default()
            },
        ]);

        let req = http::Request::builder()
            .uri("http://www.example.com")
            .header("x-foo", "bar")
            .header("x-baz", "qux")
            .header("x-biz", "qyx")
            .body(())
            .unwrap();
        let (_, rule) = rts.find_route(&req).expect("must match");
        assert!(
            rule.labels.contains_key("expected"),
            "incorrect rule matched"
        );
    }

    #[test]
    fn inbound_find_route_first_identical_wins() {
        // Given two routes with header matches, use the one that matches more
        // headers.
        let rts = InboundRoutes(vec![
            InboundRoute {
                rules: vec![
                    InboundRule {
                        labels: maplit::btreemap! {
                            "expected".to_string() => "".to_string(),
                        },
                        ..InboundRule::default()
                    },
                    // Redundant unlabeled rule.
                    InboundRule::default(),
                ],
                ..InboundRoute::default()
            },
            // Redundant unlabeled route.
            InboundRoute {
                rules: vec![InboundRule::default()],
                ..InboundRoute::default()
            },
        ]);

        let (_, rule) = rts
            .find_route(&http::Request::builder().body(()).unwrap())
            .expect("must match");
        assert!(
            rule.labels.contains_key("expected"),
            "incorrect rule matched"
        );
    }
}
