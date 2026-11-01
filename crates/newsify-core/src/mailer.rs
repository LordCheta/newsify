//! Outbound e-mail interface. `newsify-worker` implements it on the Resend
//! HTTP API; tests use a recording fake.

use crate::error::MailError;
use crate::model::Email;

#[allow(async_fn_in_trait)]
pub trait Mailer {
    /// Sends one e-mail.
    async fn send(&self, email: &Email) -> Result<(), MailError>;

    /// Sends up to 100 distinct e-mails in one call. All or nothing.
    ///
    /// `idempotency_key` identifies this exact batch: the provider returns the
    /// original result instead of sending twice if the same key and payload
    /// arrive again (for example after a Worker timeout), so retries cannot
    /// double-send.
    async fn send_batch(&self, emails: &[Email], idempotency_key: &str) -> Result<(), MailError>;
}
