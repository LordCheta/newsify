//! E-mail copy. Plain functions, no template engine: every interpolated value
//! that did not come from the site owner is HTML-escaped.

use crate::config::Config;
use crate::model::{ContactMessage, Email, PendingDelivery};
use crate::token;

/// Escapes text for safe inclusion in HTML (element content and quoted attributes).
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Plain text to simple HTML: escaped, blank lines become paragraphs.
pub fn text_to_html(text: &str) -> String {
    text.replace("\r\n", "\n")
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .map(|p| format!("<p>{}</p>", escape_html(p.trim()).replace('\n', "<br>")))
        .collect::<Vec<_>>()
        .join("\n")
}

fn wrap_html(body: &str, footer: &str) -> String {
    format!(
        "<!doctype html><html><body style=\"margin:0;padding:24px;background:#f6f6f4;\
font-family:-apple-system,Segoe UI,Helvetica,Arial,sans-serif;color:#1a1a1a;\">\
<div style=\"max-width:600px;margin:0 auto;background:#fff;padding:32px;border-radius:8px;\
line-height:1.6;font-size:16px;\">{body}</div>\
<div style=\"max-width:600px;margin:16px auto 0;font-size:12px;color:#777;line-height:1.5;\">{footer}</div>\
</body></html>"
    )
}

pub fn confirm_url(config: &Config, token: &str) -> String {
    format!("{}/confirm?t={}", config.public_base_url, token)
}

pub fn unsubscribe_url(config: &Config, subscriber_id: &str) -> String {
    let t = token::sign_unsubscribe(&config.signing_secret, subscriber_id);
    format!("{}/unsubscribe?t={}", config.public_base_url, t)
}

/// The double opt-in e-mail.
pub fn confirmation_email(config: &Config, to: &str, token: &str) -> Email {
    let url = confirm_url(config, token);
    let brand = escape_html(&config.brand_name);
    let text = format!(
        "Confirm your subscription to {brand}\n\n\
Click this link to confirm your email address:\n{url}\n\n\
The link works for 7 days. If you did not sign up, ignore this email and \
you will not be subscribed.\n",
        brand = config.brand_name,
    );
    let html = wrap_html(
        &format!(
            "<h2 style=\"margin-top:0\">Confirm your subscription</h2>\
<p>Please confirm your email address to subscribe to <strong>{brand}</strong>.</p>\
<p><a href=\"{url}\" style=\"display:inline-block;background:#1f6f4a;color:#fff;\
padding:12px 20px;border-radius:6px;text-decoration:none\">Confirm subscription</a></p>\
<p style=\"font-size:14px;color:#555\">Or paste this link into your browser:<br>{url}</p>"
        ),
        "The link works for 7 days. If you did not sign up, ignore this email and you will not be subscribed.",
    );
    Email {
        from: config.from.clone(),
        to: to.to_string(),
        subject: format!("Confirm your subscription to {}", config.brand_name),
        html,
        text,
        reply_to: None,
        headers: vec![],
    }
}

/// One issue for one subscriber: the body plus an unsubscribe footer and the
/// `List-Unsubscribe` headers (RFC 2369 and one-click, RFC 8058).
pub fn issue_email(config: &Config, delivery: &PendingDelivery) -> Email {
    let url = unsubscribe_url(config, &delivery.subscriber_id);
    let text = format!(
        "{}\n\n--\nYou are receiving this because you subscribed to {}.\nUnsubscribe: {}\n",
        delivery.text_body.trim_end(),
        config.brand_name,
        url
    );
    let footer = format!(
        "You are receiving this because you subscribed to {}. \
<a href=\"{url}\" style=\"color:#777\">Unsubscribe</a>.",
        escape_html(&config.brand_name)
    );
    Email {
        from: config.from.clone(),
        to: delivery.email.clone(),
        subject: delivery.subject.clone(),
        html: wrap_html(&delivery.html_body, &footer),
        text,
        reply_to: None,
        headers: vec![
            ("List-Unsubscribe".into(), format!("<{url}>")),
            (
                "List-Unsubscribe-Post".into(),
                "List-Unsubscribe=One-Click".into(),
            ),
        ],
    }
}

/// The notification the site owner receives for a contact-form message.
pub fn contact_notification(config: &Config, m: &ContactMessage) -> Email {
    let company = if m.company.is_empty() {
        "(not given)".to_string()
    } else {
        m.company.clone()
    };
    let text = format!(
        "New message from the {} website\n\nName: {}\nEmail: {}\nCompany: {}\n\n{}\n",
        config.brand_name, m.name, m.email, company, m.message
    );
    let html = format!(
        "<p><strong>New message from the {brand} website</strong></p>\
<p>Name: {name}<br>Email: {email}<br>Company: {company}</p>\
<blockquote style=\"margin:0;padding-left:12px;border-left:3px solid #ccc\">{body}</blockquote>",
        brand = escape_html(&config.brand_name),
        name = escape_html(&m.name),
        email = escape_html(&m.email),
        company = escape_html(&company),
        body = text_to_html(&m.message),
    );
    Email {
        from: config.from.clone(),
        to: config.contact_to.clone(),
        subject: format!("Website contact: {}", m.name),
        html,
        text,
        reply_to: Some(m.email.clone()),
        headers: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::new(
            "Acme <b>",
            "Acme <news@acme.test>",
            "https://api.acme.test/",
            "https://acme.test/",
            "owner@acme.test",
            "secret",
        )
    }

    #[test]
    fn escapes_html() {
        assert_eq!(
            escape_html("<script>\"a\" & 'b'</script>"),
            "&lt;script&gt;&quot;a&quot; &amp; &#39;b&#39;&lt;/script&gt;"
        );
    }

    #[test]
    fn text_to_html_makes_paragraphs_and_escapes() {
        assert_eq!(
            text_to_html("one\ntwo\n\n<three>"),
            "<p>one<br>two</p>\n<p>&lt;three&gt;</p>"
        );
        assert_eq!(text_to_html("\n\n  \n\n"), "");
    }

    #[test]
    fn base_urls_have_no_double_slash() {
        let c = cfg();
        assert_eq!(
            confirm_url(&c, "tok"),
            "https://api.acme.test/confirm?t=tok"
        );
    }

    #[test]
    fn confirmation_email_escapes_brand_in_html_only() {
        let e = confirmation_email(&cfg(), "x@y.test", "tok");
        assert!(e.html.contains("Acme &lt;b&gt;"));
        assert!(!e.html.contains("<strong>Acme <b>"));
        assert!(e.text.contains("https://api.acme.test/confirm?t=tok"));
    }

    #[test]
    fn issue_email_has_unsubscribe_link_and_headers() {
        let d = PendingDelivery {
            issue_id: "i".into(),
            subscriber_id: "sub1".into(),
            email: "x@y.test".into(),
            subject: "Hi".into(),
            text_body: "body".into(),
            html_body: "<p>body</p>".into(),
            attempts: 0,
        };
        let e = issue_email(&cfg(), &d);
        let url = unsubscribe_url(&cfg(), "sub1");
        assert!(e.text.contains(&url));
        assert!(e.html.contains(&url));
        assert!(e
            .headers
            .iter()
            .any(|(k, v)| k == "List-Unsubscribe" && v == &format!("<{url}>")));
        assert!(e
            .headers
            .iter()
            .any(|(k, v)| k == "List-Unsubscribe-Post" && v == "List-Unsubscribe=One-Click"));
    }

    #[test]
    fn contact_notification_escapes_visitor_input() {
        let m = ContactMessage {
            id: "1".into(),
            name: "<img src=x onerror=1>".into(),
            email: "v@x.test".into(),
            company: "".into(),
            message: "hello <b>there</b>, friend".into(),
            created_at: 0,
        };
        let e = contact_notification(&cfg(), &m);
        assert!(!e.html.contains("<img"));
        assert!(e.html.contains("&lt;img"));
        assert!(!e.html.contains("<b>there"));
        assert_eq!(e.reply_to.as_deref(), Some("v@x.test"));
        assert_eq!(e.to, "owner@acme.test");
    }
}
