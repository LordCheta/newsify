//! In-memory [`Store`] and recording [`Mailer`] / [`Platform`] fakes.
//!
//! Public (not `cfg(test)`) so the worker crate and examples can reuse them,
//! but they are test doubles, not for production use.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::error::{MailError, StoreError};
use crate::mailer::Mailer;
use crate::model::*;
use crate::store::{Store, StoreResult};
use crate::Platform;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryRow {
    pub issue_id: String,
    pub subscriber_id: String,
    /// `pending`, `sent` or `failed`.
    pub status: &'static str,
    pub attempts: u32,
    pub last_error: Option<String>,
}

#[derive(Default)]
pub struct MemoryStore {
    pub subscribers: RefCell<HashMap<String, Subscriber>>,
    pub events: RefCell<Vec<(String, String, i64)>>,
    pub contacts: RefCell<Vec<ContactMessage>>,
    pub issues: RefCell<Vec<Issue>>,
    pub deliveries: RefCell<Vec<DeliveryRow>>,
    /// Set to make every call fail, to test error paths.
    pub fail: Cell<bool>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn check(&self) -> StoreResult<()> {
        if self.fail.get() {
            Err(StoreError("injected failure".into()))
        } else {
            Ok(())
        }
    }

    pub fn subscriber(&self, email: &str) -> Option<Subscriber> {
        self.subscribers
            .borrow()
            .values()
            .find(|s| s.email == email)
            .cloned()
    }

    pub fn delivery_rows(&self) -> Vec<DeliveryRow> {
        self.deliveries.borrow().clone()
    }
}

impl Store for MemoryStore {
    async fn find_subscriber_by_email(&self, email: &str) -> StoreResult<Option<Subscriber>> {
        self.check()?;
        Ok(self.subscriber(email))
    }

    async fn find_subscriber_by_id(&self, id: &str) -> StoreResult<Option<Subscriber>> {
        self.check()?;
        Ok(self.subscribers.borrow().get(id).cloned())
    }

    async fn find_subscriber_by_token_hash(&self, hash: &str) -> StoreResult<Option<Subscriber>> {
        self.check()?;
        Ok(self
            .subscribers
            .borrow()
            .values()
            .find(|s| s.token_hash.as_deref() == Some(hash))
            .cloned())
    }

    async fn save_subscriber(&self, subscriber: &Subscriber) -> StoreResult<()> {
        self.check()?;
        let mut map = self.subscribers.borrow_mut();
        if map
            .values()
            .any(|s| s.email == subscriber.email && s.id != subscriber.id)
        {
            return Err(StoreError(
                "UNIQUE constraint failed: subscribers.email".into(),
            ));
        }
        map.insert(subscriber.id.clone(), subscriber.clone());
        Ok(())
    }

    async fn count_events(&self, kind: &str, key: &str, since: i64) -> StoreResult<u32> {
        self.check()?;
        Ok(self
            .events
            .borrow()
            .iter()
            .filter(|(k, key2, at)| k == kind && key2 == key && *at >= since)
            .count() as u32)
    }

    async fn record_event(&self, kind: &str, key: &str, at: i64) -> StoreResult<()> {
        self.check()?;
        self.events
            .borrow_mut()
            .push((kind.to_string(), key.to_string(), at));
        Ok(())
    }

    async fn insert_contact(&self, message: &ContactMessage) -> StoreResult<()> {
        self.check()?;
        self.contacts.borrow_mut().push(message.clone());
        Ok(())
    }

    async fn create_issue(&self, issue: &Issue) -> StoreResult<u32> {
        self.check()?;
        self.issues.borrow_mut().push(issue.clone());
        let mut active: Vec<_> = self
            .subscribers
            .borrow()
            .values()
            .filter(|s| s.status == SubscriberStatus::Active)
            .map(|s| s.id.clone())
            .collect();
        active.sort();
        let n = active.len() as u32;
        let mut rows = self.deliveries.borrow_mut();
        for subscriber_id in active {
            rows.push(DeliveryRow {
                issue_id: issue.id.clone(),
                subscriber_id,
                status: "pending",
                attempts: 0,
                last_error: None,
            });
        }
        Ok(n)
    }

    async fn find_issue(&self, id: &str) -> StoreResult<Option<Issue>> {
        self.check()?;
        Ok(self.issues.borrow().iter().find(|i| i.id == id).cloned())
    }

    async fn pending_deliveries(
        &self,
        issue_id: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<PendingDelivery>> {
        self.check()?;
        let subs = self.subscribers.borrow();
        let issues = self.issues.borrow();
        let mut out = Vec::new();
        for row in self.deliveries.borrow().iter() {
            if out.len() >= limit {
                break;
            }
            if row.status != "pending" || issue_id.is_some_and(|id| id != row.issue_id) {
                continue;
            }
            let Some(sub) = subs.get(&row.subscriber_id) else {
                continue;
            };
            if sub.status != SubscriberStatus::Active {
                continue;
            }
            let Some(issue) = issues.iter().find(|i| i.id == row.issue_id) else {
                continue;
            };
            out.push(PendingDelivery {
                issue_id: row.issue_id.clone(),
                subscriber_id: row.subscriber_id.clone(),
                email: sub.email.clone(),
                subject: issue.subject.clone(),
                text_body: issue.text_body.clone(),
                html_body: issue.html_body.clone(),
                attempts: row.attempts,
            });
        }
        Ok(out)
    }

    async fn mark_sent(&self, items: &[PendingDelivery], _at: i64) -> StoreResult<()> {
        self.check()?;
        for row in self.deliveries.borrow_mut().iter_mut() {
            if items
                .iter()
                .any(|d| d.issue_id == row.issue_id && d.subscriber_id == row.subscriber_id)
            {
                row.status = "sent";
                row.attempts += 1;
            }
        }
        Ok(())
    }

    async fn mark_attempt_failed(
        &self,
        items: &[PendingDelivery],
        error: &str,
        permanent: bool,
        max_attempts: u32,
        _at: i64,
    ) -> StoreResult<()> {
        self.check()?;
        for row in self.deliveries.borrow_mut().iter_mut() {
            if items
                .iter()
                .any(|d| d.issue_id == row.issue_id && d.subscriber_id == row.subscriber_id)
            {
                row.attempts += 1;
                row.last_error = Some(error.to_string());
                if permanent || row.attempts >= max_attempts {
                    row.status = "failed";
                }
            }
        }
        Ok(())
    }

    async fn issue_progress(&self, issue_id: &str) -> StoreResult<IssueProgress> {
        self.check()?;
        let subs = self.subscribers.borrow();
        let mut p = IssueProgress::default();
        for row in self
            .deliveries
            .borrow()
            .iter()
            .filter(|r| r.issue_id == issue_id)
        {
            match row.status {
                "sent" => p.sent += 1,
                "failed" => p.failed += 1,
                // Pending deliveries to people who have since unsubscribed
                // will never be sent, so they do not count as outstanding.
                _ => {
                    if subs
                        .get(&row.subscriber_id)
                        .is_some_and(|s| s.status == SubscriberStatus::Active)
                    {
                        p.pending += 1;
                    }
                }
            }
        }
        Ok(p)
    }

    async fn stats(&self) -> StoreResult<Stats> {
        self.check()?;
        let subs = self.subscribers.borrow();
        let count = |st| subs.values().filter(|s| s.status == st).count() as u32;
        Ok(Stats {
            active: count(SubscriberStatus::Active),
            pending: count(SubscriberStatus::Pending),
            unsubscribed: count(SubscriberStatus::Unsubscribed),
            issues: self.issues.borrow().len() as u32,
            contact_messages: self.contacts.borrow().len() as u32,
        })
    }
}

/// Records every e-mail instead of sending it.
#[derive(Default)]
pub struct RecordingMailer {
    pub sent: RefCell<Vec<Email>>,
    pub batches: RefCell<Vec<(String, usize)>>,
    /// Number of upcoming calls that should fail, and how.
    pub fail_next: Cell<u32>,
    pub fail_retryable: Cell<bool>,
    /// For retryable failures: whether they count as an attempt.
    pub fail_without_penalty: Cell<bool>,
}

impl RecordingMailer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sent_to(&self, to: &str) -> Vec<Email> {
        self.sent
            .borrow()
            .iter()
            .filter(|e| e.to == to)
            .cloned()
            .collect()
    }

    fn maybe_fail(&self) -> Result<(), MailError> {
        if self.fail_next.get() > 0 {
            self.fail_next.set(self.fail_next.get() - 1);
            return Err(MailError {
                message: "injected failure".into(),
                retryable: self.fail_retryable.get(),
                penalize: !self.fail_without_penalty.get(),
            });
        }
        Ok(())
    }
}

impl Mailer for RecordingMailer {
    async fn send(&self, email: &Email) -> Result<(), MailError> {
        self.maybe_fail()?;
        self.sent.borrow_mut().push(email.clone());
        Ok(())
    }

    async fn send_batch(&self, emails: &[Email], idempotency_key: &str) -> Result<(), MailError> {
        self.maybe_fail()?;
        self.batches
            .borrow_mut()
            .push((idempotency_key.to_string(), emails.len()));
        self.sent.borrow_mut().extend(emails.iter().cloned());
        Ok(())
    }
}

/// Deterministic clock and "random" source.
pub struct FakePlatform {
    pub now: Cell<i64>,
    counter: Cell<u64>,
}

impl FakePlatform {
    pub fn new(now: i64) -> Self {
        Self {
            now: Cell::new(now),
            counter: Cell::new(0),
        }
    }

    pub fn advance(&self, secs: i64) {
        self.now.set(self.now.get() + secs);
    }
}

impl Platform for FakePlatform {
    fn now_secs(&self) -> i64 {
        self.now.get()
    }

    fn random_bytes(&self, n: usize) -> Vec<u8> {
        // Unique but predictable. Never use outside tests.
        let c = self.counter.get() + 1;
        self.counter.set(c);
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            out.push(if i < 8 {
                c.to_be_bytes()[i]
            } else {
                (i as u8).wrapping_mul(37) ^ 0xA5
            });
        }
        out
    }
}
