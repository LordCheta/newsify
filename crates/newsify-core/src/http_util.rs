//! Small HTTP-level helpers that carry logic worth testing natively.

use crate::token::constant_time_eq;

/// Returns the `Access-Control-Allow-Origin` value for a request origin, or
/// `None` if the origin is not allowed. `allowed` is a comma-separated list of
/// exact origins; `*` allows any origin.
pub fn cors_origin(allowed: &str, origin: Option<&str>) -> Option<String> {
    let origin = origin?.trim();
    if origin.is_empty() {
        return None;
    }
    for entry in allowed.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        if entry == "*" {
            return Some("*".to_string());
        }
        if entry.trim_end_matches('/') == origin {
            return Some(origin.to_string());
        }
    }
    None
}

/// Checks an `Authorization: Bearer <token>` header against the admin token.
pub fn bearer_matches(header: Option<&str>, expected: &str) -> bool {
    if expected.is_empty() {
        return false; // an unset admin token must never authenticate
    }
    let Some(header) = header else { return false };
    let Some(token) = header.strip_prefix("Bearer ") else {
        return false;
    };
    constant_time_eq(token.trim(), expected)
}

/// A form-style truthy check used for the honeypot field: any non-blank value
/// means a bot filled it in.
pub fn honeypot_tripped(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cors_matches_exact_origins() {
        let allowed = "https://a.test, https://b.test/";
        assert_eq!(
            cors_origin(allowed, Some("https://a.test")).as_deref(),
            Some("https://a.test")
        );
        assert_eq!(
            cors_origin(allowed, Some("https://b.test")).as_deref(),
            Some("https://b.test")
        );
        assert_eq!(cors_origin(allowed, Some("https://evil.test")), None);
        assert_eq!(cors_origin(allowed, Some("https://a.test.evil.test")), None);
        assert_eq!(cors_origin(allowed, None), None);
        assert_eq!(cors_origin("", Some("https://a.test")), None);
    }

    #[test]
    fn cors_wildcard() {
        assert_eq!(
            cors_origin("*", Some("https://x.test")).as_deref(),
            Some("*")
        );
    }

    #[test]
    fn bearer_auth() {
        assert!(bearer_matches(Some("Bearer s3cret"), "s3cret"));
        assert!(!bearer_matches(Some("Bearer wrong"), "s3cret"));
        assert!(!bearer_matches(Some("s3cret"), "s3cret"));
        assert!(!bearer_matches(None, "s3cret"));
        assert!(!bearer_matches(Some("Bearer "), ""));
        assert!(!bearer_matches(Some("Bearer anything"), ""));
    }

    #[test]
    fn honeypot() {
        assert!(!honeypot_tripped(None));
        assert!(!honeypot_tripped(Some("  ")));
        assert!(honeypot_tripped(Some("http://spam")));
    }
}
