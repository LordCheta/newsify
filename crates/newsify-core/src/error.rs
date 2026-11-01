use thiserror::Error;

/// The input was rejected. The message is safe to show to the visitor.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("{0}")]
pub struct ValidationError(pub String);

impl ValidationError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

#[derive(Debug, Error)]
#[error("storage error: {0}")]
pub struct StoreError(pub String);

#[derive(Debug, Error)]
#[error("mail error: {message}")]
pub struct MailError {
    pub message: String,
    /// `true` for conditions worth retrying later (timeouts, 429, 5xx).
    pub retryable: bool,
    /// Whether a retryable failure counts towards the delivery's attempt
    /// limit. Rate limiting (429) and bad credentials do not: they say
    /// nothing about the e-mail itself and are fixed by waiting or by the
    /// operator, so they must not burn through a subscriber's attempts.
    pub penalize: bool,
}

impl MailError {
    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
            penalize: true,
        }
    }
    /// Retry later without counting an attempt (429, auth problems).
    pub fn transient(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
            penalize: false,
        }
    }
    pub fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
            penalize: true,
        }
    }
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error("too many requests, try again later")]
    RateLimited,
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Mail(#[from] MailError),
}
