use http::Uri;
use regex::Regex;

#[derive(Clone, Debug)]
pub enum MatchPath {
    Exact(String),
    Prefix(String),
    Regex(Regex),
}

// === impl MatchPath ===

impl MatchPath {
    pub fn match_length(&self, uri: &Uri) -> Option<usize> {
        match self {
            Self::Exact(s) => {
                if s == uri.path() {
                    return Some(s.len());
                }
            }

            Self::Regex(re) => {
                if let Some(m) = re.find(uri.path()) {
                    let len = uri.path().len();
                    if m.start() == 0 && m.end() == len {
                        return Some(len);
                    }
                }
            }

            Self::Prefix(prefix) => {
                let path = uri.path();
                if path.starts_with(prefix) {
                    let rest = &path[prefix.len()..];
                    if rest.is_empty() {
                        return Some(prefix.len());
                    }
                    if !prefix.ends_with('/') && rest.starts_with('/') {
                        return Some(prefix.len() + 1);
                    }
                }
            }
        }

        None
    }
}

impl std::hash::Hash for MatchPath {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Self::Exact(s) => s.hash(state),
            Self::Prefix(s) => s.hash(state),
            Self::Regex(r) => r.as_str().hash(state),
        };
    }
}

impl std::cmp::PartialEq for MatchPath {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Exact(s), Self::Exact(o)) => s == o,
            (Self::Prefix(s), Self::Prefix(o)) => s == o,
            (Self::Regex(s), Self::Regex(o)) => s.as_str() == o.as_str(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MatchPath;

    #[test]
    fn path_exact() {
        let m = MatchPath::Exact("/foo/bar".to_string());
        assert_eq!(
            m.match_length(&"/foo/bar".parse().unwrap()),
            Some("/foo/bar".len())
        );
        assert_eq!(m.match_length(&"/foo".parse().unwrap()), None);
        assert_eq!(m.match_length(&"/foo/bah".parse().unwrap()), None);
        assert_eq!(m.match_length(&"/foo/bar/qux".parse().unwrap()), None);
    }

    #[test]
    fn path_prefix() {
        let m = MatchPath::Prefix("/foo".to_string());
        assert_eq!(m.match_length(&"/foo".parse().unwrap()), Some("/foo".len()));
        assert_eq!(
            m.match_length(&"/foo/bar".parse().unwrap()),
            Some("/foo/".len())
        );
        assert_eq!(
            m.match_length(&"/foo/bar/qux".parse().unwrap()),
            Some("/foo/".len())
        );
        assert_eq!(m.match_length(&"/foobar".parse().unwrap()), None);
    }

    #[test]
    fn path_regex() {
        let m = MatchPath::Regex(r#"/foo/\d+"#.parse().unwrap());
        assert_eq!(
            m.match_length(&"/foo/4".parse().unwrap()),
            Some("/foo/4".len())
        );
        assert_eq!(
            m.match_length(&"/foo/4321".parse().unwrap()),
            Some("/foo/4321".len())
        );
        assert_eq!(m.match_length(&"/bar/foo/4".parse().unwrap()), None);
        assert_eq!(m.match_length(&"/foo/4abc".parse().unwrap()), None);
        assert_eq!(m.match_length(&"/foo/4/bar".parse().unwrap()), None);
        assert_eq!(m.match_length(&"/foo/bar".parse().unwrap()), None);
    }
}
