use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubscriberStatus {
    /// Signed up but has not clicked the confirmation link. Never mailed.
    Pending,
    Active,
    Unsubscribed,
}

impl SubscriberStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Unsubscribed => "unsubscribed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "active" => Some(Self::Active),
            "unsubscribed" => Some(Self::Unsubscribed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscriber {
    pub id: String,
    pub email: String,
    pub status: SubscriberStatus,
    /// SHA-256 of the confirmation token. The token itself is only ever in the
    /// e-mail, so a database leak cannot be used to confirm addresses.
    pub token_hash: Option<String>,
    pub token_created_at: Option<i64>,
    pub created_at: i64,
    pub confirmed_at: Option<i64>,
    pub unsubscribed_at: Option<i64>,
    /// Where the signup came from (free text, e.g. "website").
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub id: String,
    pub subject: String,
    pub text_body: String,
    pub html_body: String,
    pub created_at: i64,
}

/// One e-mail still to be sent: an issue paired with a subscriber.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDelivery {
    pub issue_id: String,
    pub subscriber_id: String,
    pub email: String,
    pub subject: String,
    pub text_body: String,
    pub html_body: String,
    pub attempts: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct IssueProgress {
    pub pending: u32,
    pub sent: u32,
    pub failed: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Stats {
    pub active: u32,
    pub pending: u32,
    pub unsubscribed: u32,
    pub issues: u32,
    pub contact_messages: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactMessage {
    pub id: String,
    pub name: String,
    pub email: String,
    pub company: String,
    pub message: String,
    pub created_at: i64,
}

/// A fully built outbound e-mail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Email {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub html: String,
    pub text: String,
    pub reply_to: Option<String>,
    pub headers: Vec<(String, String)>,
}
