//! [`Mailer`] on the Resend HTTP API (<https://resend.com/docs/api-reference>),
//! plus a log-only mode for local development.

use newsify_core::error::MailError;
use newsify_core::mailer::Mailer;
use newsify_core::model::Email;
use serde_json::{json, Value};
use worker::wasm_bindgen::JsValue;
use worker::{console_log, Fetch, Headers, Method, Request, RequestInit};

pub enum EmailMode {
    /// Send through Resend.
    Resend { api_key: String },
    /// Print e-mails to the Worker log instead of sending. For local dev only.
    Log,
}

pub struct ResendMailer {
    pub mode: EmailMode,
}

const API: &str = "https://api.resend.com";

fn payload(e: &Email) -> Value {
    let headers: serde_json::Map<String, Value> = e
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    let mut v = json!({
        "from": e.from,
        "to": [e.to],
        "subject": e.subject,
        "html": e.html,
        "text": e.text,
    });
    if let Some(reply_to) = &e.reply_to {
        v["reply_to"] = json!(reply_to);
    }
    if !headers.is_empty() {
        v["headers"] = Value::Object(headers);
    }
    v
}

/// Maps an HTTP status from Resend to a retry decision.
pub fn classify(status: u16, body: &str) -> MailError {
    let msg = format!(
        "resend returned {status}: {}",
        body.chars().take(300).collect::<String>()
    );
    match status {
        // Rate limited, or another request with the same idempotency key is
        // still running: wait and try again, without penalty.
        429 => MailError::transient(msg),
        409 if body.contains("concurrent_idempotent_requests") => MailError::transient(msg),
        // Our credentials or sender domain are wrong: an operator problem,
        // not a problem with the subscriber's address.
        401 | 403 => MailError::transient(msg),
        500..=599 => MailError::retryable(msg),
        _ => MailError::permanent(msg),
    }
}

impl ResendMailer {
    async fn post(
        &self,
        path: &str,
        body: &Value,
        idempotency_key: Option<&str>,
    ) -> Result<(), MailError> {
        let EmailMode::Resend { api_key } = &self.mode else {
            unreachable!("post is only called in Resend mode");
        };
        let headers = Headers::new();
        let bad = |e: worker::Error| MailError::retryable(e.to_string());
        headers
            .set("Authorization", &format!("Bearer {api_key}"))
            .map_err(bad)?;
        headers
            .set("Content-Type", "application/json")
            .map_err(bad)?;
        if let Some(key) = idempotency_key {
            headers.set("Idempotency-Key", key).map_err(bad)?;
        }
        let mut init = RequestInit::new();
        init.with_method(Method::Post)
            .with_headers(headers)
            .with_body(Some(JsValue::from_str(&body.to_string())));
        let request = Request::new_with_init(&format!("{API}{path}"), &init).map_err(bad)?;
        let mut response = Fetch::Request(request).send().await.map_err(bad)?;
        let status = response.status_code();
        if (200..300).contains(&status) {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        Err(classify(status, &text))
    }
}

impl Mailer for ResendMailer {
    async fn send(&self, email: &Email) -> Result<(), MailError> {
        match &self.mode {
            EmailMode::Log => {
                console_log!(
                    "[newsify:email] to={} subject={:?}\n{}",
                    email.to,
                    email.subject,
                    email.text
                );
                Ok(())
            }
            EmailMode::Resend { .. } => self.post("/emails", &payload(email), None).await,
        }
    }

    async fn send_batch(&self, emails: &[Email], idempotency_key: &str) -> Result<(), MailError> {
        match &self.mode {
            EmailMode::Log => {
                for e in emails {
                    console_log!(
                        "[newsify:email] to={} subject={:?}\n{}",
                        e.to,
                        e.subject,
                        e.text
                    );
                }
                Ok(())
            }
            EmailMode::Resend { .. } => {
                let body = Value::Array(emails.iter().map(payload).collect());
                self.post("/emails/batch", &body, Some(idempotency_key))
                    .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limits_and_bad_credentials_do_not_penalize() {
        for status in [401, 403, 429] {
            let e = classify(status, "");
            assert!(e.retryable && !e.penalize, "status {status}");
        }
        let e = classify(409, r#"{"name":"concurrent_idempotent_requests"}"#);
        assert!(e.retryable && !e.penalize);
    }

    #[test]
    fn server_errors_retry_and_validation_errors_do_not() {
        let e = classify(503, "");
        assert!(e.retryable && e.penalize);
        for status in [400, 422, 409] {
            assert!(!classify(status, "").retryable, "status {status}");
        }
    }

    #[test]
    fn payload_carries_unsubscribe_headers_and_reply_to() {
        let mut headers = std::collections::BTreeMap::new();
        headers.insert("List-Unsubscribe".to_string(), "<https://x/u>".to_string());
        let v = payload(&Email {
            from: "a@b.c".into(),
            to: "d@e.f".into(),
            subject: "s".into(),
            html: "<p>h</p>".into(),
            text: "t".into(),
            reply_to: Some("r@e.f".into()),
            headers: headers.into_iter().collect(),
        });
        assert_eq!(v["to"][0], "d@e.f");
        assert_eq!(v["reply_to"], "r@e.f");
        assert_eq!(v["headers"]["List-Unsubscribe"], "<https://x/u>");
    }
}
