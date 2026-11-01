//! Input validation. Deliberately strict: a newsletter only needs ordinary
//! addresses, and rejecting exotic ones removes a class of header-injection
//! and parsing problems.

use crate::error::ValidationError;

pub const MAX_NAME: usize = 100;
pub const MAX_COMPANY: usize = 100;
pub const MIN_MESSAGE: usize = 10;
pub const MAX_MESSAGE: usize = 5_000;
pub const MAX_SUBJECT: usize = 200;
pub const MAX_BODY: usize = 200_000;

/// Trims, lower-cases and validates an e-mail address.
pub fn normalize_email(raw: &str) -> Result<String, ValidationError> {
    let email = raw.trim().to_ascii_lowercase();
    let invalid = || ValidationError::new("Please enter a valid email address.");

    if email.is_empty() || email.len() > 254 {
        return Err(invalid());
    }
    let (local, domain) = email.split_once('@').ok_or_else(invalid)?;
    if local.is_empty() || local.len() > 64 || domain.contains('@') {
        return Err(invalid());
    }
    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return Err(invalid());
    }
    let local_ok = local
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+/=?^_`{|}~.-".contains(c));
    if !local_ok {
        return Err(invalid());
    }
    if !domain.contains('.') || domain.len() > 253 {
        return Err(invalid());
    }
    for label in domain.split('.') {
        let ok = !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !ok {
            return Err(invalid());
        }
    }
    // The top-level domain is never numeric (rules out IP literals like a@1.2.3.4).
    if domain
        .rsplit('.')
        .next()
        .is_some_and(|tld| tld.chars().all(|c| c.is_ascii_digit()) || tld.len() < 2)
    {
        return Err(invalid());
    }
    Ok(email)
}

fn has_control(s: &str) -> bool {
    s.chars().any(|c| c.is_control())
}

/// Single-line free text such as a name: trimmed, no control characters.
pub fn single_line(
    field: &str,
    raw: &str,
    max: usize,
    required: bool,
) -> Result<String, ValidationError> {
    let value = raw.trim();
    if value.is_empty() {
        return if required {
            Err(ValidationError::new(format!("{field} is required.")))
        } else {
            Ok(String::new())
        };
    }
    if value.chars().count() > max {
        return Err(ValidationError::new(format!(
            "{field} must be at most {max} characters."
        )));
    }
    if has_control(value) {
        return Err(ValidationError::new(format!(
            "{field} contains invalid characters."
        )));
    }
    Ok(value.to_string())
}

/// Multi-line free text: newlines and tabs allowed, other control characters not.
pub fn multi_line(
    field: &str,
    raw: &str,
    min: usize,
    max: usize,
) -> Result<String, ValidationError> {
    let value = raw.trim();
    let len = value.chars().count();
    if len < min {
        return Err(ValidationError::new(format!(
            "{field} must be at least {min} characters."
        )));
    }
    if len > max {
        return Err(ValidationError::new(format!(
            "{field} must be at most {max} characters."
        )));
    }
    if value
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
    {
        return Err(ValidationError::new(format!(
            "{field} contains invalid characters."
        )));
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_and_normalizes_ordinary_addresses() {
        assert_eq!(
            normalize_email("  Ada.Lovelace+news@Example.CO.uk ").unwrap(),
            "ada.lovelace+news@example.co.uk"
        );
        assert!(normalize_email("a@b.io").is_ok());
    }

    #[test]
    fn rejects_malformed_addresses() {
        for bad in [
            "",
            "plain",
            "@example.com",
            "a@",
            "a@b",
            "a@@b.com",
            "a b@example.com",
            "a@exa mple.com",
            ".a@example.com",
            "a.@example.com",
            "a..b@example.com",
            "a@-example.com",
            "a@example-.com",
            "a@example..com",
            "a@1.2.3.4",
            "a@example.c",
            "a@exämple.com",
            "a\r\nBcc: x@y.com@example.com",
            "<a@example.com>",
        ] {
            assert!(normalize_email(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn rejects_overlong_addresses() {
        let long_local = format!("{}@example.com", "a".repeat(65));
        assert!(normalize_email(&long_local).is_err());
        let long_domain = format!("a@{}.com", "b".repeat(250));
        assert!(normalize_email(&long_domain).is_err());
    }

    #[test]
    fn single_line_rules() {
        assert_eq!(single_line("Name", "  Ada ", 10, true).unwrap(), "Ada");
        assert!(single_line("Name", "   ", 10, true).is_err());
        assert_eq!(single_line("Company", "", 10, false).unwrap(), "");
        assert!(single_line("Name", "Ada\nBcc: x", 50, true).is_err());
        assert!(single_line("Name", "12345678901", 10, true).is_err());
    }

    #[test]
    fn multi_line_rules() {
        assert!(multi_line("Message", "short", 10, 100).is_err());
        assert!(multi_line("Message", "line one\nline two", 10, 100).is_ok());
        assert!(multi_line("Message", "bad \u{0} byte here", 10, 100).is_err());
        assert!(multi_line("Message", &"x".repeat(101), 10, 100).is_err());
    }
}
