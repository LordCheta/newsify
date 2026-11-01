//! Platform-independent core of **newsify**.
//!
//! Everything that decides *what should happen* lives here: validating input,
//! the double opt-in state machine, signed unsubscribe links, rate limiting,
//! building e-mails and batching an issue out to subscribers.
//!
//! Everything that touches the outside world sits behind three small traits,
//! so this crate compiles and is tested natively with in-memory fakes, while
//! `newsify-worker` plugs in Cloudflare D1, the Resend API and the Workers
//! runtime:
//!
//! * [`store::Store`] for persistence
//! * [`mailer::Mailer`] for outbound e-mail
//! * [`Platform`] for the clock and randomness

pub mod config;
pub mod error;
pub mod http_util;
pub mod mailer;
pub mod memory;
pub mod model;
pub mod service;
pub mod store;
pub mod templates;
pub mod token;
pub mod validate;

/// Clock and randomness, supplied by the host so the core stays deterministic
/// under test and free of OS or JavaScript dependencies.
pub trait Platform {
    /// Current Unix time in seconds.
    fn now_secs(&self) -> i64;
    /// `n` cryptographically secure random bytes.
    fn random_bytes(&self, n: usize) -> Vec<u8>;
}
