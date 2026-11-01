//! Cloudflare Turnstile server-side verification.

use serde::Deserialize;
use worker::wasm_bindgen::JsValue;
use worker::{Fetch, Headers, Method, Request, RequestInit};

#[derive(Deserialize)]
struct SiteVerify {
    success: bool,
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Returns whether the token is valid. `Err` means Turnstile could not be
/// reached, which callers should not treat as "the visitor is a bot".
pub async fn verify(secret: &str, token: &str, ip: Option<&str>) -> worker::Result<bool> {
    if token.is_empty() {
        return Ok(false);
    }
    let mut form = format!("secret={}&response={}", encode(secret), encode(token));
    if let Some(ip) = ip {
        form.push_str(&format!("&remoteip={}", encode(ip)));
    }
    let headers = Headers::new();
    headers.set("Content-Type", "application/x-www-form-urlencoded")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(JsValue::from_str(&form)));
    let request = Request::new_with_init(
        "https://challenges.cloudflare.com/turnstile/v0/siteverify",
        &init,
    )?;
    let mut response = Fetch::Request(request).send().await?;
    let parsed: SiteVerify = response.json().await?;
    Ok(parsed.success)
}

#[cfg(test)]
mod tests {
    use super::encode;

    #[test]
    fn encodes_reserved_characters() {
        assert_eq!(encode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(encode("AZaz09-_.~"), "AZaz09-_.~");
    }
}
