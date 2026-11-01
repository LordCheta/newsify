//! [`Store`] on Cloudflare D1.

use newsify_core::error::StoreError;
use newsify_core::model::*;
use newsify_core::store::{Store, StoreResult};
use serde::Deserialize;
use worker::wasm_bindgen::JsValue;
use worker::{D1Database, D1PreparedStatement};

pub struct D1Store {
    db: D1Database,
}

impl D1Store {
    pub fn new(db: D1Database) -> Self {
        Self { db }
    }

    fn stmt(&self, sql: &str, args: &[JsValue]) -> StoreResult<D1PreparedStatement> {
        self.db.prepare(sql).bind(args).map_err(map_err)
    }
}

fn map_err(e: worker::Error) -> StoreError {
    StoreError(e.to_string())
}

fn s(v: &str) -> JsValue {
    JsValue::from_str(v)
}

fn n(v: i64) -> JsValue {
    JsValue::from_f64(v as f64)
}

fn opt_s(v: &Option<String>) -> JsValue {
    v.as_deref().map(JsValue::from_str).unwrap_or(JsValue::NULL)
}

fn opt_n(v: Option<i64>) -> JsValue {
    v.map(n).unwrap_or(JsValue::NULL)
}

#[derive(Deserialize)]
struct SubscriberRow {
    id: String,
    email: String,
    status: String,
    token_hash: Option<String>,
    token_created_at: Option<i64>,
    created_at: i64,
    confirmed_at: Option<i64>,
    unsubscribed_at: Option<i64>,
    source: String,
}

impl SubscriberRow {
    fn into_model(self) -> StoreResult<Subscriber> {
        Ok(Subscriber {
            status: SubscriberStatus::parse(&self.status).ok_or_else(|| {
                StoreError(format!("unknown subscriber status {:?}", self.status))
            })?,
            id: self.id,
            email: self.email,
            token_hash: self.token_hash,
            token_created_at: self.token_created_at,
            created_at: self.created_at,
            confirmed_at: self.confirmed_at,
            unsubscribed_at: self.unsubscribed_at,
            source: self.source,
        })
    }
}

#[derive(Deserialize)]
struct CountRow {
    n: u32,
}

#[derive(Deserialize)]
struct QueueRow {
    issue_id: String,
    subscriber_id: String,
    email: String,
    attempts: u32,
}

#[derive(Deserialize)]
struct IssueRow {
    id: String,
    subject: String,
    text_body: String,
    html_body: String,
    created_at: i64,
}

#[derive(Deserialize)]
struct ProgressRow {
    pending: u32,
    sent: u32,
    failed: u32,
}

#[derive(Deserialize)]
struct StatsRow {
    active: u32,
    pending: u32,
    unsubscribed: u32,
    issues: u32,
    contact_messages: u32,
}

impl D1Store {
    async fn one_subscriber(&self, sql: &str, arg: &str) -> StoreResult<Option<Subscriber>> {
        let row = self
            .stmt(sql, &[s(arg)])?
            .first::<SubscriberRow>(None)
            .await
            .map_err(map_err)?;
        row.map(SubscriberRow::into_model).transpose()
    }
}

impl Store for D1Store {
    async fn find_subscriber_by_email(&self, email: &str) -> StoreResult<Option<Subscriber>> {
        self.one_subscriber("SELECT * FROM subscribers WHERE email = ?1", email)
            .await
    }

    async fn find_subscriber_by_id(&self, id: &str) -> StoreResult<Option<Subscriber>> {
        self.one_subscriber("SELECT * FROM subscribers WHERE id = ?1", id)
            .await
    }

    async fn find_subscriber_by_token_hash(&self, hash: &str) -> StoreResult<Option<Subscriber>> {
        self.one_subscriber("SELECT * FROM subscribers WHERE token_hash = ?1", hash)
            .await
    }

    async fn save_subscriber(&self, x: &Subscriber) -> StoreResult<()> {
        self.stmt(
            "INSERT INTO subscribers
               (id, email, status, token_hash, token_created_at, created_at, confirmed_at, unsubscribed_at, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
               email = excluded.email,
               status = excluded.status,
               token_hash = excluded.token_hash,
               token_created_at = excluded.token_created_at,
               confirmed_at = excluded.confirmed_at,
               unsubscribed_at = excluded.unsubscribed_at",
            &[
                s(&x.id),
                s(&x.email),
                s(x.status.as_str()),
                opt_s(&x.token_hash),
                opt_n(x.token_created_at),
                n(x.created_at),
                opt_n(x.confirmed_at),
                opt_n(x.unsubscribed_at),
                s(&x.source),
            ],
        )?
        .run()
        .await
        .map_err(map_err)?;
        Ok(())
    }

    async fn count_events(&self, kind: &str, key: &str, since: i64) -> StoreResult<u32> {
        let row = self
            .stmt(
                "SELECT COUNT(*) AS n FROM rate_events WHERE kind = ?1 AND key = ?2 AND created_at >= ?3",
                &[s(kind), s(key), n(since)],
            )?
            .first::<CountRow>(None)
            .await
            .map_err(map_err)?;
        Ok(row.map(|r| r.n).unwrap_or(0))
    }

    async fn record_event(&self, kind: &str, key: &str, at: i64) -> StoreResult<()> {
        // Also drop events older than a day so the table cannot grow forever.
        let statements = vec![
            self.stmt(
                "INSERT INTO rate_events (kind, key, created_at) VALUES (?1, ?2, ?3)",
                &[s(kind), s(key), n(at)],
            )?,
            self.stmt(
                "DELETE FROM rate_events WHERE created_at < ?1",
                &[n(at - 86_400)],
            )?,
        ];
        self.db.batch(statements).await.map_err(map_err)?;
        Ok(())
    }

    async fn insert_contact(&self, m: &ContactMessage) -> StoreResult<()> {
        self.stmt(
            "INSERT INTO contact_messages (id, name, email, company, message, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            &[
                s(&m.id),
                s(&m.name),
                s(&m.email),
                s(&m.company),
                s(&m.message),
                n(m.created_at),
            ],
        )?
        .run()
        .await
        .map_err(map_err)?;
        Ok(())
    }

    async fn create_issue(&self, issue: &Issue) -> StoreResult<u32> {
        // One transactional batch: the issue and its whole recipient list are
        // created together, from the subscribers who are active right now.
        let statements = vec![
            self.stmt(
                "INSERT INTO issues (id, subject, text_body, html_body, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                &[
                    s(&issue.id),
                    s(&issue.subject),
                    s(&issue.text_body),
                    s(&issue.html_body),
                    n(issue.created_at),
                ],
            )?,
            self.stmt(
                "INSERT INTO deliveries (issue_id, subscriber_id, status, attempts, updated_at)
                 SELECT ?1, id, 'pending', 0, ?2 FROM subscribers WHERE status = 'active'",
                &[s(&issue.id), n(issue.created_at)],
            )?,
        ];
        self.db.batch(statements).await.map_err(map_err)?;
        let row = self
            .stmt(
                "SELECT COUNT(*) AS n FROM deliveries WHERE issue_id = ?1",
                &[s(&issue.id)],
            )?
            .first::<CountRow>(None)
            .await
            .map_err(map_err)?;
        Ok(row.map(|r| r.n).unwrap_or(0))
    }

    async fn find_issue(&self, id: &str) -> StoreResult<Option<Issue>> {
        let row = self
            .stmt("SELECT * FROM issues WHERE id = ?1", &[s(id)])?
            .first::<IssueRow>(None)
            .await
            .map_err(map_err)?;
        Ok(row.map(|r| Issue {
            id: r.id,
            subject: r.subject,
            text_body: r.text_body,
            html_body: r.html_body,
            created_at: r.created_at,
        }))
    }

    async fn pending_deliveries(
        &self,
        issue_id: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<PendingDelivery>> {
        let issue_arg = issue_id.map(s).unwrap_or(JsValue::NULL);
        let rows = self
            .stmt(
                "SELECT d.issue_id, d.subscriber_id, s.email, d.attempts
                 FROM deliveries d
                 JOIN subscribers s ON s.id = d.subscriber_id
                 JOIN issues i ON i.id = d.issue_id
                 WHERE d.status = 'pending' AND s.status = 'active'
                   AND (?1 IS NULL OR d.issue_id = ?1)
                 ORDER BY i.created_at, d.rowid
                 LIMIT ?2",
                &[issue_arg, n(limit as i64)],
            )?
            .all()
            .await
            .map_err(map_err)?
            .results::<QueueRow>()
            .map_err(map_err)?;

        // Fetch each distinct issue once instead of repeating a large body on
        // every row.
        let mut issues: Vec<Issue> = Vec::new();
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            if !issues.iter().any(|i| i.id == row.issue_id) {
                if let Some(issue) = self.find_issue(&row.issue_id).await? {
                    issues.push(issue);
                }
            }
            let Some(issue) = issues.iter().find(|i| i.id == row.issue_id) else {
                continue;
            };
            out.push(PendingDelivery {
                issue_id: row.issue_id,
                subscriber_id: row.subscriber_id,
                email: row.email,
                subject: issue.subject.clone(),
                text_body: issue.text_body.clone(),
                html_body: issue.html_body.clone(),
                attempts: row.attempts,
            });
        }
        Ok(out)
    }

    async fn mark_sent(&self, items: &[PendingDelivery], at: i64) -> StoreResult<()> {
        let mut statements = Vec::with_capacity(items.len());
        for d in items {
            statements.push(self.stmt(
                "UPDATE deliveries
                 SET status = 'sent', attempts = attempts + 1, last_error = NULL, updated_at = ?3
                 WHERE issue_id = ?1 AND subscriber_id = ?2 AND status = 'pending'",
                &[s(&d.issue_id), s(&d.subscriber_id), n(at)],
            )?);
        }
        self.db.batch(statements).await.map_err(map_err)?;
        Ok(())
    }

    async fn mark_attempt_failed(
        &self,
        items: &[PendingDelivery],
        error: &str,
        permanent: bool,
        max_attempts: u32,
        at: i64,
    ) -> StoreResult<()> {
        let error: String = error.chars().take(500).collect();
        let mut statements = Vec::with_capacity(items.len());
        for d in items {
            statements.push(self.stmt(
                "UPDATE deliveries
                 SET attempts = attempts + 1,
                     last_error = ?3,
                     updated_at = ?4,
                     status = CASE WHEN ?5 = 1 OR attempts + 1 >= ?6 THEN 'failed' ELSE 'pending' END
                 WHERE issue_id = ?1 AND subscriber_id = ?2 AND status = 'pending'",
                &[
                    s(&d.issue_id),
                    s(&d.subscriber_id),
                    s(&error),
                    n(at),
                    n(permanent as i64),
                    n(max_attempts as i64),
                ],
            )?);
        }
        self.db.batch(statements).await.map_err(map_err)?;
        Ok(())
    }

    async fn issue_progress(&self, issue_id: &str) -> StoreResult<IssueProgress> {
        let row = self
            .stmt(
                "SELECT
                   COALESCE(SUM(CASE WHEN d.status = 'pending' AND s.status = 'active' THEN 1 ELSE 0 END), 0) AS pending,
                   COALESCE(SUM(CASE WHEN d.status = 'sent' THEN 1 ELSE 0 END), 0) AS sent,
                   COALESCE(SUM(CASE WHEN d.status = 'failed' THEN 1 ELSE 0 END), 0) AS failed
                 FROM deliveries d JOIN subscribers s ON s.id = d.subscriber_id
                 WHERE d.issue_id = ?1",
                &[s(issue_id)],
            )?
            .first::<ProgressRow>(None)
            .await
            .map_err(map_err)?;
        Ok(row
            .map(|r| IssueProgress {
                pending: r.pending,
                sent: r.sent,
                failed: r.failed,
            })
            .unwrap_or_default())
    }

    async fn stats(&self) -> StoreResult<Stats> {
        let row = self
            .stmt(
                "SELECT
                   (SELECT COUNT(*) FROM subscribers WHERE status = 'active') AS active,
                   (SELECT COUNT(*) FROM subscribers WHERE status = 'pending') AS pending,
                   (SELECT COUNT(*) FROM subscribers WHERE status = 'unsubscribed') AS unsubscribed,
                   (SELECT COUNT(*) FROM issues) AS issues,
                   (SELECT COUNT(*) FROM contact_messages) AS contact_messages",
                &[],
            )?
            .first::<StatsRow>(None)
            .await
            .map_err(map_err)?;
        let r = row.ok_or_else(|| StoreError("stats query returned no row".into()))?;
        Ok(Stats {
            active: r.active,
            pending: r.pending,
            unsubscribed: r.unsubscribed,
            issues: r.issues,
            contact_messages: r.contact_messages,
        })
    }
}
