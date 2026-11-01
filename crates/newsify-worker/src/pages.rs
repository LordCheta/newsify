//! Tiny self-contained HTML pages for the unsubscribe flow.

use newsify_core::templates::escape_html;

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<meta name=\"robots\" content=\"noindex\"><title>{title}</title>\
<style>body{{font-family:system-ui,sans-serif;background:#f6f7f9;color:#1c1f24;margin:0;\
display:grid;place-items:center;min-height:100vh}}main{{background:#fff;padding:2rem;\
border-radius:12px;max-width:26rem;box-shadow:0 2px 12px rgba(0,0,0,.08)}}\
button{{font:inherit;padding:.6rem 1.2rem;border:0;border-radius:8px;background:#1c1f24;\
color:#fff;cursor:pointer}}a{{color:inherit}}</style></head><body><main>{body}</main></body></html>",
        title = escape_html(title)
    )
}

pub fn confirm_unsubscribe(token: &str) -> String {
    let t = escape_html(token);
    page(
        "Unsubscribe",
        &format!(
            "<h1>Unsubscribe</h1><p>Do you want to stop receiving this newsletter?</p>\
<form method=\"post\" action=\"/unsubscribe?t={t}\"><button type=\"submit\">Yes, unsubscribe me</button></form>"
        ),
    )
}

pub fn unsubscribed(brand: &str, site_url: &str) -> String {
    page(
        "Unsubscribed",
        &format!(
            "<h1>You are unsubscribed</h1><p>You will not receive any more email from {}.</p>\
<p><a href=\"{}\">Back to the site</a></p>",
            escape_html(brand),
            escape_html(site_url)
        ),
    )
}

pub fn invalid_unsubscribe(site_url: &str) -> String {
    page(
        "Invalid link",
        &format!(
            "<h1>This link is not valid</h1><p>The unsubscribe link is incomplete or damaged. \
Please use the link from the latest email, or reply to it and we will remove you by hand.</p>\
<p><a href=\"{}\">Back to the site</a></p>",
            escape_html(site_url)
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_escaped_in_the_form_action() {
        let html = confirm_unsubscribe("\"><script>alert(1)</script>");
        assert!(!html.contains("<script>"));
    }
}
