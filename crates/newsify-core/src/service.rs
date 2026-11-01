//! The application logic. Free of I/O: it only talks to [`Store`], [`Mailer`]
//! and [`Platform`].

use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::error::{MailError, ServiceError, StoreError, ValidationError};
use crate::mailer::Mailer;
use crate::model::*;
use crate::store::Store;
use crate::templates;
use crate::token;
use crate::validate;
use crate::Platform;

const HOUR: i64 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscribeOutcome {
    /// A confirmation e-mail was sent.
    ConfirmationSent,
    /// Already pending and a confirmation was sent very recently; nothing sent.
    RecentlySent,
    /// Already an active subscriber; nothing sent.
    AlreadySubscribed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmOutcome {
    Confirmed,
    AlreadyConfirmed,
    /// Unknown token, or the address has since unsubscribed.
    Invalid,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsubscribeOutcome {
    Done,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactInput {
    pub name: String,
    pub email: String,
    pub company: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContactOutcome {
    /// `false` if the message was saved but the owner notification failed.
    pub notified: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct SendReport {
    pub sent: u32,
    pub failed: u32,
    /// Deliveries that hit a temporary error and will be retried.
    pub retry_later: u32,
    pub batches: u32,
}

pub struct Newsify<'a, S: Store, M: Mailer, P: Platform> {
    pub store: &'a S,
    pub mailer: &'a M,
    pub platform: &'a P,
    pub config: &'a Config,
}

impl<'a, S: Store, M: Mailer, P: Platform> Newsify<'a, S, M, P> {
    pub fn new(store: &'a S, mailer: &'a M, platform: &'a P, config: &'a Config) -> Self {
        Self {
            store,
            mailer,
            platform,
            config,
        }
    }

    fn ip_key(&self, ip: Option<&str>) -> String {
        token::hash_ip(&self.config.signing_secret, ip.unwrap_or("unknown"))
    }

    /// Counts this attempt and refuses when the IP is over its hourly limit.
    async fn rate_limit(
        &self,
        kind: &str,
        ip: Option<&str>,
        limit: u32,
    ) -> Result<(), ServiceError> {
        let now = self.platform.now_secs();
        let key = self.ip_key(ip);
        if self.store.count_events(kind, &key, now - HOUR).await? >= limit {
            return Err(ServiceError::RateLimited);
        }
        self.store.record_event(kind, &key, now).await?;
        Ok(())
    }

    // ---------------------------------------------------------------- subscribe

    /// Starts (or restarts) a double opt-in subscription.
    ///
    /// Nobody is ever mailed an issue until they click the confirmation link.
    /// Callers should answer identically for every [`SubscribeOutcome`], so the
    /// endpoint cannot be used to find out who is subscribed.
    pub async fn subscribe(
        &self,
        raw_email: &str,
        ip: Option<&str>,
        source: &str,
    ) -> Result<SubscribeOutcome, ServiceError> {
        let email = validate::normalize_email(raw_email)?;
        self.rate_limit("subscribe", ip, self.config.subscribe_per_hour)
            .await?;
        let now = self.platform.now_secs();

        let existing = self.store.find_subscriber_by_email(&email).await?;
        let mut subscriber = match existing {
            Some(s) if s.status == SubscriberStatus::Active => {
                return Ok(SubscribeOutcome::AlreadySubscribed)
            }
            Some(s)
                if s.status == SubscriberStatus::Pending
                    && s.token_created_at
                        .is_some_and(|t| now - t < self.config.confirm_cooldown_secs) =>
            {
                return Ok(SubscribeOutcome::RecentlySent)
            }
            Some(s) => s,
            None => Subscriber {
                id: token::generate_id(self.platform),
                email: email.clone(),
                status: SubscriberStatus::Pending,
                token_hash: None,
                token_created_at: None,
                created_at: now,
                confirmed_at: None,
                unsubscribed_at: None,
                source: source.chars().take(50).collect(),
            },
        };

        let confirm_token = token::generate_token(self.platform);
        subscriber.status = SubscriberStatus::Pending;
        subscriber.token_hash = Some(token::hash_token(&confirm_token));
        subscriber.token_created_at = Some(now);
        subscriber.confirmed_at = None;
        subscriber.unsubscribed_at = None;
        self.store.save_subscriber(&subscriber).await?;

        let mail = templates::confirmation_email(self.config, &email, &confirm_token);
        if let Err(e) = self.mailer.send(&mail).await {
            // Let the visitor try again straight away instead of waiting out
            // the cooldown for an e-mail that never left.
            subscriber.token_created_at = None;
            let _ = self.store.save_subscriber(&subscriber).await;
            return Err(e.into());
        }
        Ok(SubscribeOutcome::ConfirmationSent)
    }

    /// Completes the double opt-in. Idempotent: clicking twice is fine.
    pub async fn confirm(&self, token: &str) -> Result<ConfirmOutcome, ServiceError> {
        let token = token.trim();
        if token.is_empty() || token.len() > 128 {
            return Ok(ConfirmOutcome::Invalid);
        }
        let hash = token::hash_token(token);
        let Some(mut s) = self.store.find_subscriber_by_token_hash(&hash).await? else {
            return Ok(ConfirmOutcome::Invalid);
        };
        match s.status {
            SubscriberStatus::Active => Ok(ConfirmOutcome::AlreadyConfirmed),
            SubscriberStatus::Unsubscribed => Ok(ConfirmOutcome::Invalid),
            SubscriberStatus::Pending => {
                let now = self.platform.now_secs();
                let fresh = s
                    .token_created_at
                    .is_some_and(|t| now - t <= self.config.confirm_ttl_secs);
                if !fresh {
                    return Ok(ConfirmOutcome::Expired);
                }
                s.status = SubscriberStatus::Active;
                s.confirmed_at = Some(now);
                self.store.save_subscriber(&s).await?;
                Ok(ConfirmOutcome::Confirmed)
            }
        }
    }

    /// Verifies a signed unsubscribe token and opts the subscriber out. Idempotent.
    pub async fn unsubscribe(&self, token: &str) -> Result<UnsubscribeOutcome, ServiceError> {
        let Some(id) = token::verify_unsubscribe(&self.config.signing_secret, token.trim()) else {
            return Ok(UnsubscribeOutcome::Invalid);
        };
        let Some(mut s) = self.store.find_subscriber_by_id(&id).await? else {
            return Ok(UnsubscribeOutcome::Done);
        };
        if s.status != SubscriberStatus::Unsubscribed {
            s.status = SubscriberStatus::Unsubscribed;
            s.unsubscribed_at = Some(self.platform.now_secs());
            self.store.save_subscriber(&s).await?;
        }
        Ok(UnsubscribeOutcome::Done)
    }

    // ------------------------------------------------------------------ issues

    /// Stores an issue and queues it for every active subscriber. Nothing is
    /// sent yet; call [`Self::send_batches`].
    ///
    /// `html` is trusted (it comes from the site owner) and is inserted as is.
    /// Without it, the plain text body is converted to simple, escaped HTML.
    pub async fn create_issue(
        &self,
        subject: &str,
        text: &str,
        html: Option<&str>,
    ) -> Result<(Issue, u32), ServiceError> {
        let subject = validate::single_line("Subject", subject, validate::MAX_SUBJECT, true)?;
        let text = validate::multi_line("Body", text, 1, validate::MAX_BODY)?;
        let html_body = match html.map(str::trim).filter(|h| !h.is_empty()) {
            Some(h) if h.len() > validate::MAX_BODY => {
                return Err(ValidationError::new("HTML body is too long.").into())
            }
            Some(h) => h.to_string(),
            None => templates::text_to_html(&text),
        };
        let issue = Issue {
            id: token::generate_id(self.platform),
            subject,
            text_body: text,
            html_body,
            created_at: self.platform.now_secs(),
        };
        let recipients = self.store.create_issue(&issue).await?;
        Ok((issue, recipients))
    }

    /// Sends up to `max_batches` batches of pending deliveries, then returns.
    ///
    /// Safe to call repeatedly and concurrently with itself: batches carry a
    /// provider idempotency key derived from their exact contents, so a retry
    /// after a timeout cannot double-send. Subscribers who unsubscribe while an
    /// issue is going out are skipped.
    pub async fn send_batches(
        &self,
        issue_id: Option<&str>,
        max_batches: u32,
    ) -> Result<SendReport, ServiceError> {
        let mut report = SendReport::default();
        for _ in 0..max_batches {
            let items = self
                .store
                .pending_deliveries(issue_id, self.config.batch_size.clamp(1, 100))
                .await?;
            if items.is_empty() {
                break;
            }
            report.batches += 1;
            let emails: Vec<Email> = items
                .iter()
                .map(|d| templates::issue_email(self.config, d))
                .collect();
            let key = batch_key(&items);

            match self.mailer.send_batch(&emails, &key).await {
                Ok(()) => {
                    self.store
                        .mark_sent(&items, self.platform.now_secs())
                        .await?;
                    report.sent += items.len() as u32;
                }
                Err(e) if e.retryable && !e.penalize => {
                    // Rate limited or misconfigured: leave everything pending
                    // and untouched, and try again on the next run.
                    report.retry_later += items.len() as u32;
                    break;
                }
                Err(e) if e.retryable => {
                    self.fail_items(&items, &e, &mut report).await?;
                    // The provider is struggling: stop and let the next run retry.
                    break;
                }
                Err(e) => {
                    // A batch is all-or-nothing, so one bad address would block
                    // everyone in it. Send individually to isolate the culprit.
                    for (item, email) in items.iter().zip(&emails) {
                        match self.mailer.send(email).await {
                            Ok(()) => {
                                self.store
                                    .mark_sent(std::slice::from_ref(item), self.platform.now_secs())
                                    .await?;
                                report.sent += 1;
                            }
                            Err(e2) => {
                                self.fail_items(std::slice::from_ref(item), &e2, &mut report)
                                    .await?;
                            }
                        }
                    }
                    let _ = e;
                }
            }
        }
        Ok(report)
    }

    async fn fail_items(
        &self,
        items: &[PendingDelivery],
        e: &MailError,
        report: &mut SendReport,
    ) -> Result<(), StoreError> {
        let permanent = !e.retryable;
        self.store
            .mark_attempt_failed(
                items,
                &e.message,
                permanent,
                self.config.max_attempts,
                self.platform.now_secs(),
            )
            .await?;
        for item in items {
            if permanent || item.attempts + 1 >= self.config.max_attempts {
                report.failed += 1;
            } else {
                report.retry_later += 1;
            }
        }
        Ok(())
    }

    pub async fn issue_progress(
        &self,
        issue_id: &str,
    ) -> Result<Option<(Issue, IssueProgress)>, StoreError> {
        let Some(issue) = self.store.find_issue(issue_id).await? else {
            return Ok(None);
        };
        let progress = self.store.issue_progress(issue_id).await?;
        Ok(Some((issue, progress)))
    }

    pub async fn stats(&self) -> Result<Stats, StoreError> {
        self.store.stats().await
    }

    // ----------------------------------------------------------------- contact

    /// Stores a contact-form message and e-mails it to the site owner.
    ///
    /// The message is saved first, so it is not lost if the e-mail fails; in
    /// that case the visitor still sees success and `notified` is `false`.
    pub async fn contact(
        &self,
        input: &ContactInput,
        ip: Option<&str>,
    ) -> Result<ContactOutcome, ServiceError> {
        let name = validate::single_line("Name", &input.name, validate::MAX_NAME, true)?;
        let email = validate::normalize_email(&input.email)?;
        let company =
            validate::single_line("Company", &input.company, validate::MAX_COMPANY, false)?;
        let message = validate::multi_line(
            "Message",
            &input.message,
            validate::MIN_MESSAGE,
            validate::MAX_MESSAGE,
        )?;
        self.rate_limit("contact", ip, self.config.contact_per_hour)
            .await?;

        let record = ContactMessage {
            id: token::generate_id(self.platform),
            name,
            email,
            company,
            message,
            created_at: self.platform.now_secs(),
        };
        self.store.insert_contact(&record).await?;
        let mail = templates::contact_notification(self.config, &record);
        let notified = self.mailer.send(&mail).await.is_ok();
        Ok(ContactOutcome { notified })
    }
}

/// Idempotency key for one exact batch: the same set of deliveries always
/// yields the same key, and any difference yields a different one.
fn batch_key(items: &[PendingDelivery]) -> String {
    let mut pairs: Vec<String> = items
        .iter()
        .map(|d| format!("{}:{}", d.issue_id, d.subscriber_id))
        .collect();
    pairs.sort();
    let digest = hex::encode(Sha256::digest(pairs.join(",").as_bytes()));
    format!("newsify/{}", &digest[..40])
}
