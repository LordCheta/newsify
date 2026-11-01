//! Persistence interface. `newsify-worker` implements it on Cloudflare D1;
//! [`crate::memory::MemoryStore`] implements it in memory for tests.

use crate::error::StoreError;
use crate::model::*;

pub type StoreResult<T> = Result<T, StoreError>;

#[allow(async_fn_in_trait)]
pub trait Store {
    async fn find_subscriber_by_email(&self, email: &str) -> StoreResult<Option<Subscriber>>;
    async fn find_subscriber_by_id(&self, id: &str) -> StoreResult<Option<Subscriber>>;
    async fn find_subscriber_by_token_hash(&self, hash: &str) -> StoreResult<Option<Subscriber>>;
    /// Insert, or update the row with the same `id`.
    async fn save_subscriber(&self, subscriber: &Subscriber) -> StoreResult<()>;

    /// Number of events of `kind` for `key` at or after `since` (rate limiting).
    async fn count_events(&self, kind: &str, key: &str, since: i64) -> StoreResult<u32>;
    async fn record_event(&self, kind: &str, key: &str, at: i64) -> StoreResult<()>;

    async fn insert_contact(&self, message: &ContactMessage) -> StoreResult<()>;

    /// Stores the issue and queues one delivery per **active** subscriber at
    /// this moment. Returns the number of queued deliveries.
    async fn create_issue(&self, issue: &Issue) -> StoreResult<u32>;
    async fn find_issue(&self, id: &str) -> StoreResult<Option<Issue>>;
    /// Up to `limit` deliveries still to send (oldest issue first), for
    /// subscribers who are still active. Restricted to one issue when given.
    async fn pending_deliveries(
        &self,
        issue_id: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<PendingDelivery>>;
    async fn mark_sent(&self, items: &[PendingDelivery], at: i64) -> StoreResult<()>;
    /// Records a failed attempt. A delivery is permanently `failed` once it
    /// has reached `max_attempts` or when `permanent` is set.
    async fn mark_attempt_failed(
        &self,
        items: &[PendingDelivery],
        error: &str,
        permanent: bool,
        max_attempts: u32,
        at: i64,
    ) -> StoreResult<()>;
    async fn issue_progress(&self, issue_id: &str) -> StoreResult<IssueProgress>;
    async fn stats(&self) -> StoreResult<Stats>;
}
