//! End-to-end behaviour of the service against in-memory fakes.

use futures::executor::block_on;
use newsify_core::config::Config;
use newsify_core::error::ServiceError;
use newsify_core::memory::{FakePlatform, MemoryStore, RecordingMailer};
use newsify_core::model::SubscriberStatus;
use newsify_core::service::*;
use newsify_core::store::Store;

struct Harness {
    store: MemoryStore,
    mailer: RecordingMailer,
    platform: FakePlatform,
    config: Config,
}

impl Harness {
    fn new() -> Self {
        let mut config = Config::new(
            "Acme",
            "Acme <news@acme.test>",
            "https://api.acme.test",
            "https://acme.test",
            "owner@acme.test",
            "test-signing-secret",
        );
        config.batch_size = 3;
        Self {
            store: MemoryStore::new(),
            mailer: RecordingMailer::new(),
            platform: FakePlatform::new(1_000_000),
            config,
        }
    }

    fn svc(&self) -> Newsify<'_, MemoryStore, RecordingMailer, FakePlatform> {
        Newsify::new(&self.store, &self.mailer, &self.platform, &self.config)
    }

    /// Subscribe and confirm an address; returns nothing, asserts it is active.
    fn add_active(&self, email: &str) {
        // A distinct "IP" per address so the rate limiter does not interfere.
        block_on(self.svc().subscribe(email, Some(email), "test")).unwrap();
        let token = self.last_confirm_token(email);
        assert_eq!(
            block_on(self.svc().confirm(&token)).unwrap(),
            ConfirmOutcome::Confirmed
        );
    }

    fn last_confirm_token(&self, to: &str) -> String {
        let mail = self
            .mailer
            .sent_to(to)
            .pop()
            .expect("a confirmation e-mail");
        let url = mail
            .text
            .lines()
            .find(|l| l.contains("/confirm?t="))
            .expect("confirm link");
        url.split("t=").nth(1).unwrap().trim().to_string()
    }

    fn unsubscribe_token(&self, email: &str) -> String {
        let id = self.store.subscriber(email).unwrap().id;
        newsify_core::token::sign_unsubscribe(&self.config.signing_secret, &id)
    }
}

// ------------------------------------------------------------------ subscribe

#[test]
fn double_opt_in_happy_path() {
    let h = Harness::new();
    let out = block_on(
        h.svc()
            .subscribe("Ada@Example.com", Some("1.1.1.1"), "website"),
    )
    .unwrap();
    assert_eq!(out, SubscribeOutcome::ConfirmationSent);

    // Stored as pending, e-mail normalised, token only stored hashed.
    let sub = h.store.subscriber("ada@example.com").unwrap();
    assert_eq!(sub.status, SubscriberStatus::Pending);
    let token = h.last_confirm_token("ada@example.com");
    assert_ne!(sub.token_hash.as_deref(), Some(token.as_str()));
    assert_eq!(
        sub.token_hash.unwrap(),
        newsify_core::token::hash_token(&token)
    );

    // A pending subscriber is never sent an issue.
    let (_, recipients) = block_on(h.svc().create_issue("Hi", "Hello", None)).unwrap();
    assert_eq!(recipients, 0);

    assert_eq!(
        block_on(h.svc().confirm(&token)).unwrap(),
        ConfirmOutcome::Confirmed
    );
    assert_eq!(
        h.store.subscriber("ada@example.com").unwrap().status,
        SubscriberStatus::Active
    );
    // Confirming again is harmless.
    assert_eq!(
        block_on(h.svc().confirm(&token)).unwrap(),
        ConfirmOutcome::AlreadyConfirmed
    );
}

#[test]
fn rejects_invalid_email_without_side_effects() {
    let h = Harness::new();
    let err = block_on(h.svc().subscribe("not-an-email", Some("1.1.1.1"), "t")).unwrap_err();
    assert!(matches!(err, ServiceError::Validation(_)));
    assert!(h.mailer.sent.borrow().is_empty());
    assert!(h.store.subscribers.borrow().is_empty());
}

#[test]
fn repeat_signup_is_throttled_then_resent() {
    let h = Harness::new();
    let s = || block_on(h.svc().subscribe("a@example.com", Some("1.1.1.1"), "t")).unwrap();
    assert_eq!(s(), SubscribeOutcome::ConfirmationSent);
    assert_eq!(s(), SubscribeOutcome::RecentlySent);
    assert_eq!(
        h.mailer.sent.borrow().len(),
        1,
        "no second e-mail inside the cooldown"
    );

    h.platform.advance(h.config.confirm_cooldown_secs + 1);
    assert_eq!(s(), SubscribeOutcome::ConfirmationSent);
    assert_eq!(h.mailer.sent.borrow().len(), 2);
}

#[test]
fn old_confirmation_token_stops_working_after_resend() {
    let h = Harness::new();
    block_on(h.svc().subscribe("a@example.com", Some("1.1.1.1"), "t")).unwrap();
    let old = h.last_confirm_token("a@example.com");
    h.platform.advance(h.config.confirm_cooldown_secs + 1);
    block_on(h.svc().subscribe("a@example.com", Some("1.1.1.1"), "t")).unwrap();
    assert_eq!(
        block_on(h.svc().confirm(&old)).unwrap(),
        ConfirmOutcome::Invalid
    );
    let new = h.last_confirm_token("a@example.com");
    assert_eq!(
        block_on(h.svc().confirm(&new)).unwrap(),
        ConfirmOutcome::Confirmed
    );
}

#[test]
fn already_subscribed_sends_nothing() {
    let h = Harness::new();
    h.add_active("a@example.com");
    let before = h.mailer.sent.borrow().len();
    let out = block_on(h.svc().subscribe("a@example.com", Some("2.2.2.2"), "t")).unwrap();
    assert_eq!(out, SubscribeOutcome::AlreadySubscribed);
    assert_eq!(h.mailer.sent.borrow().len(), before);
}

#[test]
fn confirmation_expires() {
    let h = Harness::new();
    block_on(h.svc().subscribe("a@example.com", Some("1.1.1.1"), "t")).unwrap();
    let token = h.last_confirm_token("a@example.com");
    h.platform.advance(h.config.confirm_ttl_secs + 1);
    assert_eq!(
        block_on(h.svc().confirm(&token)).unwrap(),
        ConfirmOutcome::Expired
    );
    assert_eq!(
        h.store.subscriber("a@example.com").unwrap().status,
        SubscriberStatus::Pending
    );
}

#[test]
fn confirm_rejects_garbage() {
    let h = Harness::new();
    for t in ["", "   ", "nope", &"x".repeat(500)] {
        assert_eq!(
            block_on(h.svc().confirm(t)).unwrap(),
            ConfirmOutcome::Invalid
        );
    }
}

#[test]
fn subscribe_is_rate_limited_per_ip() {
    let h = Harness::new();
    for i in 0..h.config.subscribe_per_hour {
        block_on(
            h.svc()
                .subscribe(&format!("u{i}@example.com"), Some("9.9.9.9"), "t"),
        )
        .unwrap();
    }
    let err = block_on(h.svc().subscribe("late@example.com", Some("9.9.9.9"), "t")).unwrap_err();
    assert!(matches!(err, ServiceError::RateLimited));
    // A different IP is unaffected, and the window slides.
    assert!(block_on(h.svc().subscribe("other@example.com", Some("8.8.8.8"), "t")).is_ok());
    h.platform.advance(3601);
    assert!(block_on(h.svc().subscribe("late@example.com", Some("9.9.9.9"), "t")).is_ok());
}

#[test]
fn mail_failure_does_not_lock_the_visitor_out() {
    let h = Harness::new();
    h.mailer.fail_next.set(1);
    h.mailer.fail_retryable.set(true);
    let err = block_on(h.svc().subscribe("a@example.com", Some("1.1.1.1"), "t")).unwrap_err();
    assert!(matches!(err, ServiceError::Mail(_)));
    // Retry immediately: not blocked by the cooldown.
    let out = block_on(h.svc().subscribe("a@example.com", Some("1.1.1.1"), "t")).unwrap();
    assert_eq!(out, SubscribeOutcome::ConfirmationSent);
}

// ---------------------------------------------------------------- unsubscribe

#[test]
fn unsubscribe_flow_and_resubscribe() {
    let h = Harness::new();
    h.add_active("a@example.com");
    let t = h.unsubscribe_token("a@example.com");

    assert_eq!(
        block_on(h.svc().unsubscribe(&t)).unwrap(),
        UnsubscribeOutcome::Done
    );
    let sub = h.store.subscriber("a@example.com").unwrap();
    assert_eq!(sub.status, SubscriberStatus::Unsubscribed);
    assert!(sub.unsubscribed_at.is_some());
    // Idempotent.
    assert_eq!(
        block_on(h.svc().unsubscribe(&t)).unwrap(),
        UnsubscribeOutcome::Done
    );

    // Old confirmation links cannot resurrect an unsubscribed address.
    let old_confirm = h.last_confirm_token("a@example.com");
    assert_eq!(
        block_on(h.svc().confirm(&old_confirm)).unwrap(),
        ConfirmOutcome::Invalid
    );

    // They may sign up again, which restarts double opt-in.
    h.platform.advance(1);
    let out = block_on(h.svc().subscribe("a@example.com", Some("1.1.1.1"), "t")).unwrap();
    assert_eq!(out, SubscribeOutcome::ConfirmationSent);
    assert_eq!(
        h.store.subscriber("a@example.com").unwrap().status,
        SubscriberStatus::Pending
    );
}

#[test]
fn unsubscribe_rejects_forged_tokens() {
    let h = Harness::new();
    h.add_active("a@example.com");
    let id = h.store.subscriber("a@example.com").unwrap().id;
    for t in [
        format!("{id}.{}", "0".repeat(64)),
        id.clone(),
        String::new(),
        newsify_core::token::sign_unsubscribe("some-other-secret", &id),
    ] {
        assert_eq!(
            block_on(h.svc().unsubscribe(&t)).unwrap(),
            UnsubscribeOutcome::Invalid
        );
    }
    assert_eq!(
        h.store.subscriber("a@example.com").unwrap().status,
        SubscriberStatus::Active
    );
}

// --------------------------------------------------------------------- issues

#[test]
fn issue_goes_only_to_active_subscribers_in_batches() {
    let h = Harness::new();
    for i in 0..7 {
        h.add_active(&format!("u{i}@example.com"));
    }
    // One pending and one unsubscribed address must be skipped.
    block_on(
        h.svc()
            .subscribe("pending@example.com", Some("3.3.3.3"), "t"),
    )
    .unwrap();
    h.add_active("gone@example.com");
    let t = h.unsubscribe_token("gone@example.com");
    block_on(h.svc().unsubscribe(&t)).unwrap();
    h.mailer.sent.borrow_mut().clear();

    let (issue, recipients) = block_on(h.svc().create_issue(
        "October update",
        "Hello\n\nWorld <b>",
        None,
    ))
    .unwrap();
    assert_eq!(recipients, 7);
    assert!(
        issue.html_body.contains("&lt;b&gt;"),
        "plain text is escaped"
    );

    // batch_size = 3, so two batches send 6, a third sends the last one.
    let r1 = block_on(h.svc().send_batches(Some(&issue.id), 2)).unwrap();
    assert_eq!((r1.sent, r1.batches), (6, 2));
    let p = block_on(h.store.issue_progress(&issue.id)).unwrap();
    assert_eq!((p.sent, p.pending), (6, 1));
    let r2 = block_on(h.svc().send_batches(Some(&issue.id), 2)).unwrap();
    assert_eq!((r2.sent, r2.batches), (1, 1));
    let r3 = block_on(h.svc().send_batches(Some(&issue.id), 2)).unwrap();
    assert_eq!(
        (r3.sent, r3.batches),
        (0, 0),
        "nothing left, nothing re-sent"
    );

    let sent = h.mailer.sent.borrow();
    assert_eq!(sent.len(), 7);
    assert!(sent
        .iter()
        .all(|e| e.to != "pending@example.com" && e.to != "gone@example.com"));
    // Each recipient gets their own working unsubscribe link and headers.
    for e in sent.iter() {
        assert!(e.text.contains("/unsubscribe?t="));
        assert!(e.headers.iter().any(|(k, _)| k == "List-Unsubscribe"));
    }
    let unique: std::collections::HashSet<_> = sent.iter().map(|e| e.to.clone()).collect();
    assert_eq!(unique.len(), 7, "no recipient mailed twice");
}

#[test]
fn unsubscribing_mid_send_stops_delivery() {
    let h = Harness::new();
    for i in 0..4 {
        h.add_active(&format!("u{i}@example.com"));
    }
    let (issue, _) = block_on(h.svc().create_issue("Hi", "Hello", None)).unwrap();
    block_on(h.svc().send_batches(Some(&issue.id), 1)).unwrap(); // sends 3
    let remaining: Vec<_> = h
        .store
        .delivery_rows()
        .into_iter()
        .filter(|r| r.status == "pending")
        .collect();
    assert_eq!(remaining.len(), 1);
    let sub = h
        .store
        .subscribers
        .borrow()
        .get(&remaining[0].subscriber_id)
        .cloned()
        .unwrap();
    let t = h.unsubscribe_token(&sub.email);
    block_on(h.svc().unsubscribe(&t)).unwrap();

    let r = block_on(h.svc().send_batches(Some(&issue.id), 5)).unwrap();
    assert_eq!(r.sent, 0);
    assert!(h
        .mailer
        .sent_to(&sub.email)
        .iter()
        .all(|e| !e.subject.eq("Hi")));
    let p = block_on(h.store.issue_progress(&issue.id)).unwrap();
    assert_eq!(
        p.pending, 0,
        "an unsubscribed recipient is not 'outstanding'"
    );
}

#[test]
fn batches_carry_a_stable_idempotency_key() {
    let h = Harness::new();
    for i in 0..3 {
        h.add_active(&format!("u{i}@example.com"));
    }
    let (issue, _) = block_on(h.svc().create_issue("Hi", "Hello", None)).unwrap();

    // First attempt hits a temporary provider error; second succeeds.
    h.mailer.fail_next.set(1);
    h.mailer.fail_retryable.set(true);
    let r1 = block_on(h.svc().send_batches(Some(&issue.id), 3)).unwrap();
    assert_eq!((r1.sent, r1.retry_later), (0, 3));
    assert!(h.mailer.batches.borrow().is_empty());
    let r2 = block_on(h.svc().send_batches(Some(&issue.id), 3)).unwrap();
    assert_eq!(r2.sent, 3);

    // Same contents produce the same key, so a replayed request is deduplicated
    // by the provider.
    let key_after_retry = h.mailer.batches.borrow()[0].0.clone();
    let (issue2, _) = block_on(h.svc().create_issue("Other", "Body", None)).unwrap();
    block_on(h.svc().send_batches(Some(&issue2.id), 1)).unwrap();
    let key_other_issue = h.mailer.batches.borrow()[1].0.clone();
    assert_ne!(key_after_retry, key_other_issue);
    assert!(key_after_retry.starts_with("newsify/"));
    assert!(key_after_retry.len() <= 256);
}

#[test]
fn temporary_failures_retry_then_give_up() {
    let h = Harness::new();
    h.add_active("a@example.com");
    let (issue, _) = block_on(h.svc().create_issue("Hi", "Hello", None)).unwrap();
    h.mailer.fail_next.set(100);
    h.mailer.fail_retryable.set(true);

    for attempt in 1..=h.config.max_attempts {
        let r = block_on(h.svc().send_batches(Some(&issue.id), 1)).unwrap();
        if attempt < h.config.max_attempts {
            assert_eq!((r.failed, r.retry_later), (0, 1), "attempt {attempt}");
        } else {
            assert_eq!((r.failed, r.retry_later), (1, 0), "gives up at the limit");
        }
    }
    let p = block_on(h.store.issue_progress(&issue.id)).unwrap();
    assert_eq!((p.pending, p.failed, p.sent), (0, 1, 0));
    // A failed delivery is not retried forever.
    let r = block_on(h.svc().send_batches(Some(&issue.id), 1)).unwrap();
    assert_eq!(r.batches, 0);
}

#[test]
fn rate_limiting_by_the_provider_does_not_burn_attempts() {
    let h = Harness::new();
    h.add_active("a@example.com");
    let (issue, _) = block_on(h.svc().create_issue("Hi", "Hello", None)).unwrap();
    h.mailer.fail_next.set(100);
    h.mailer.fail_retryable.set(true);
    h.mailer.fail_without_penalty.set(true);

    for _ in 0..(h.config.max_attempts * 3) {
        let r = block_on(h.svc().send_batches(Some(&issue.id), 1)).unwrap();
        assert_eq!((r.sent, r.failed, r.retry_later), (0, 0, 1));
    }
    let row = &h.store.delivery_rows()[0];
    assert_eq!((row.status, row.attempts), ("pending", 0));

    // Once the provider recovers it goes out.
    h.mailer.fail_next.set(0);
    let r = block_on(h.svc().send_batches(Some(&issue.id), 1)).unwrap();
    assert_eq!(r.sent, 1);
}

#[test]
fn bad_address_in_a_batch_does_not_block_the_rest() {
    let h = Harness::new();
    for i in 0..3 {
        h.add_active(&format!("u{i}@example.com"));
    }
    let (issue, _) = block_on(h.svc().create_issue("Hi", "Hello", None)).unwrap();
    // The batch call is rejected permanently (e.g. one invalid address); the
    // service falls back to one-by-one. Fail only the first single send too.
    h.mailer.fail_next.set(2);
    h.mailer.fail_retryable.set(false);
    let r = block_on(h.svc().send_batches(Some(&issue.id), 1)).unwrap();
    assert_eq!((r.sent, r.failed), (2, 1));
    let p = block_on(h.store.issue_progress(&issue.id)).unwrap();
    assert_eq!((p.sent, p.failed, p.pending), (2, 1, 0));
}

#[test]
fn issue_validation() {
    let h = Harness::new();
    assert!(block_on(h.svc().create_issue("", "body", None)).is_err());
    assert!(block_on(h.svc().create_issue("Subject\nBcc: x", "body", None)).is_err());
    assert!(block_on(h.svc().create_issue("Subject", "   ", None)).is_err());
    // Owner supplied HTML is kept as is.
    let (issue, _) = block_on(h.svc().create_issue("S", "text", Some("<h1>Hi</h1>"))).unwrap();
    assert_eq!(issue.html_body, "<h1>Hi</h1>");
}

// -------------------------------------------------------------------- contact

fn contact_input() -> ContactInput {
    ContactInput {
        name: "Grace Hopper".into(),
        email: "Grace@Navy.test".into(),
        company: "Navy".into(),
        message: "We need a compiler built, yesterday.".into(),
    }
}

#[test]
fn contact_is_stored_and_forwarded_with_reply_to() {
    let h = Harness::new();
    let out = block_on(h.svc().contact(&contact_input(), Some("1.1.1.1"))).unwrap();
    assert!(out.notified);
    assert_eq!(h.store.contacts.borrow().len(), 1);
    assert_eq!(h.store.contacts.borrow()[0].email, "grace@navy.test");
    let mail = h.mailer.sent_to("owner@acme.test").pop().unwrap();
    assert_eq!(mail.reply_to.as_deref(), Some("grace@navy.test"));
    assert!(mail.text.contains("We need a compiler"));
}

#[test]
fn contact_survives_a_notification_failure() {
    let h = Harness::new();
    h.mailer.fail_next.set(1);
    let out = block_on(h.svc().contact(&contact_input(), Some("1.1.1.1"))).unwrap();
    assert!(!out.notified);
    assert_eq!(h.store.contacts.borrow().len(), 1, "message is not lost");
}

#[test]
fn contact_validation_and_rate_limit() {
    let h = Harness::new();
    let mut bad = contact_input();
    bad.email = "nope".into();
    assert!(matches!(
        block_on(h.svc().contact(&bad, Some("1.1.1.1"))),
        Err(ServiceError::Validation(_))
    ));
    let mut short = contact_input();
    short.message = "hi".into();
    assert!(block_on(h.svc().contact(&short, Some("1.1.1.1"))).is_err());
    let mut injected = contact_input();
    injected.name = "Eve\r\nBcc: victim@x.test".into();
    assert!(block_on(h.svc().contact(&injected, Some("1.1.1.1"))).is_err());
    assert!(h.store.contacts.borrow().is_empty());

    for _ in 0..h.config.contact_per_hour {
        block_on(h.svc().contact(&contact_input(), Some("5.5.5.5"))).unwrap();
    }
    assert!(matches!(
        block_on(h.svc().contact(&contact_input(), Some("5.5.5.5"))),
        Err(ServiceError::RateLimited)
    ));
}

#[test]
fn stats_count_each_state() {
    let h = Harness::new();
    h.add_active("a@example.com");
    h.add_active("b@example.com");
    block_on(h.svc().subscribe("c@example.com", Some("4.4.4.4"), "t")).unwrap();
    let t = h.unsubscribe_token("b@example.com");
    block_on(h.svc().unsubscribe(&t)).unwrap();
    let s = block_on(h.svc().stats()).unwrap();
    assert_eq!((s.active, s.pending, s.unsubscribed), (1, 1, 1));
}

#[test]
fn storage_errors_surface_instead_of_panicking() {
    let h = Harness::new();
    h.store.fail.set(true);
    assert!(matches!(
        block_on(h.svc().subscribe("a@example.com", Some("1.1.1.1"), "t")),
        Err(ServiceError::Store(_))
    ));
}
