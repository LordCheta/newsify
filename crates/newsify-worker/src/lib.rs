//! Cloudflare Worker adapter for **newsify**.
//!
//! All decisions live in `newsify-core`. This crate only translates HTTP into
//! calls on it and plugs in D1 (`d1.rs`), Resend (`resend.rs`) and the
//! Workers runtime (`platform.rs`).

mod d1;
mod pages;
mod platform;
mod resend;
mod turnstile;

use newsify_core::config::Config;
use newsify_core::error::ServiceError;
use newsify_core::http_util::{bearer_matches, cors_origin, honeypot_tripped};
use newsify_core::service::{
    ConfirmOutcome, ContactInput, Newsify, SubscribeOutcome, UnsubscribeOutcome,
};
use serde::Deserialize;
use serde_json::{json, Value};
use worker::*;

use crate::d1::D1Store;
use crate::platform::WorkerPlatform;
use crate::resend::{EmailMode, ResendMailer};

const MAX_PUBLIC_BODY: usize = 20 * 1024;
const MAX_ADMIN_BODY: usize = 600 * 1024;
/// Free-plan Workers allow 50 subrequests per invocation; each batch costs
/// about 3 (D1 reads/writes are bindings, but are counted conservatively here
/// together with the Resend call), so cap manual sends well below that.
const MAX_ADMIN_BATCHES: u32 = 5;

struct App {
    config: Config,
    store: D1Store,
    mailer: ResendMailer,
    platform: WorkerPlatform,
    allowed_origins: String,
    admin_token: String,
    turnstile_secret: Option<String>,
}

impl App {
    fn from_env(env: &Env) -> Result<Self> {
        let var = |name: &str| -> Result<String> { Ok(env.var(name)?.to_string()) };
        let secret = |name: &str| -> Result<String> { Ok(env.secret(name)?.to_string()) };
        let optional_secret = |name: &str| env.secret(name).ok().map(|s| s.to_string());

        let signing_secret = secret("SIGNING_SECRET")?;
        if signing_secret.len() < 16 {
            return Err(Error::RustError(
                "SIGNING_SECRET must be at least 16 characters".into(),
            ));
        }
        let mut config = Config::new(
            var("BRAND_NAME")?,
            var("FROM_EMAIL")?,
            var("PUBLIC_BASE_URL")?,
            var("SITE_URL")?,
            var("CONTACT_TO")?,
            signing_secret,
        );
        if let Some(n) = env
            .var("BATCH_SIZE")
            .ok()
            .and_then(|v| v.to_string().parse().ok())
        {
            config.batch_size = n;
        }

        let mode = match var("EMAIL_MODE")?.as_str() {
            "log" => EmailMode::Log,
            "resend" => EmailMode::Resend {
                api_key: secret("RESEND_API_KEY")?,
            },
            other => {
                return Err(Error::RustError(format!(
                    "EMAIL_MODE must be `resend` or `log`, got {other:?}"
                )))
            }
        };

        Ok(Self {
            config,
            store: D1Store::new(env.d1("DB")?),
            mailer: ResendMailer { mode },
            platform: WorkerPlatform,
            allowed_origins: var("ALLOWED_ORIGINS")?,
            admin_token: optional_secret("ADMIN_TOKEN").unwrap_or_default(),
            turnstile_secret: optional_secret("TURNSTILE_SECRET").filter(|s| !s.is_empty()),
        })
    }

    fn svc(&self) -> Newsify<'_, D1Store, ResendMailer, WorkerPlatform> {
        Newsify::new(&self.store, &self.mailer, &self.platform, &self.config)
    }
}

#[event(fetch)]
pub async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let app = match App::from_env(&env) {
        Ok(app) => app,
        Err(e) => {
            // Misconfiguration: tell the operator in the log, not the client.
            console_error!("configuration error: {e}");
            return json_error(500, "The service is not configured correctly.");
        }
    };
    match route(&app, req).await {
        Ok(response) => Ok(response),
        Err(e) => {
            console_error!("unhandled error: {e}");
            json_error(500, "Something went wrong.")
        }
    }
}

/// Drains the delivery queue: one batch per cron tick.
#[event(scheduled)]
pub async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    let app = match App::from_env(&env) {
        Ok(app) => app,
        Err(e) => {
            console_error!("configuration error: {e}");
            return;
        }
    };
    match app.svc().send_batches(None, 1).await {
        Ok(r) if r.batches > 0 => {
            console_log!(
                "cron: sent={} failed={} retry_later={}",
                r.sent,
                r.failed,
                r.retry_later
            );
        }
        Ok(_) => {}
        Err(e) => console_error!("cron send failed: {e}"),
    }
}

async fn route(app: &App, req: Request) -> Result<Response> {
    let url = req.url()?;
    let path = url.path().trim_end_matches('/').to_string();
    let method = req.method();

    match (method, path.as_str()) {
        (Method::Get, "/health") => Response::from_json(&json!({"ok": true})),
        (Method::Options, p) if p.starts_with("/api/") => preflight(app, &req),
        (Method::Post, "/api/subscribe") => cors(app, &req, subscribe(app, req.clone()?).await?),
        (Method::Post, "/api/contact") => cors(app, &req, contact(app, req.clone()?).await?),
        (Method::Get, "/confirm") => confirm(app, &url).await,
        (Method::Get, "/unsubscribe") => unsubscribe_page(&url),
        (Method::Post, "/unsubscribe") => unsubscribe(app, &url).await,
        (m, p) if p.starts_with("/admin/") => admin(app, m, p, &url, req).await,
        _ => json_error(404, "Not found."),
    }
}

// ------------------------------------------------------------------ helpers

fn json_response(status: u16, body: Value) -> Result<Response> {
    Ok(Response::from_json(&body)?.with_status(status))
}

fn json_error(status: u16, message: &str) -> Result<Response> {
    json_response(status, json!({"ok": false, "error": message}))
}

fn preflight(app: &App, req: &Request) -> Result<Response> {
    let origin = req.headers().get("Origin")?;
    let Some(allowed) = cors_origin(&app.allowed_origins, origin.as_deref()) else {
        return Ok(Response::empty()?.with_status(403));
    };
    let mut resp = Response::empty()?.with_status(204);
    let h = resp.headers_mut();
    h.set("Access-Control-Allow-Origin", &allowed)?;
    h.set("Access-Control-Allow-Methods", "POST, OPTIONS")?;
    h.set("Access-Control-Allow-Headers", "Content-Type")?;
    h.set("Access-Control-Max-Age", "86400")?;
    h.set("Vary", "Origin")?;
    Ok(resp)
}

fn cors(app: &App, req: &Request, mut resp: Response) -> Result<Response> {
    let origin = req.headers().get("Origin")?;
    if let Some(allowed) = cors_origin(&app.allowed_origins, origin.as_deref()) {
        let h = resp.headers_mut();
        h.set("Access-Control-Allow-Origin", &allowed)?;
        h.set("Vary", "Origin")?;
    }
    Ok(resp)
}

fn client_ip(req: &Request) -> Option<String> {
    req.headers().get("CF-Connecting-IP").ok().flatten()
}

/// Reads a JSON body with a size limit. Errors are ready-to-send responses.
async fn read_json<T: for<'de> Deserialize<'de>>(
    mut req: Request,
    limit: usize,
) -> std::result::Result<T, Result<Response>> {
    if let Ok(Some(len)) = req.headers().get("Content-Length") {
        if len.parse::<usize>().map(|n| n > limit).unwrap_or(false) {
            return Err(json_error(413, "Request body is too large."));
        }
    }
    let text = req
        .text()
        .await
        .map_err(|_| json_error(400, "Could not read the request body."))?;
    if text.len() > limit {
        return Err(json_error(413, "Request body is too large."));
    }
    serde_json::from_str(&text).map_err(|_| json_error(400, "Request body must be valid JSON."))
}

fn service_error(e: ServiceError) -> Result<Response> {
    match e {
        ServiceError::Validation(v) => json_error(400, &v.to_string()),
        ServiceError::RateLimited => json_error(429, "Too many requests, please try again later."),
        ServiceError::Store(e) => {
            console_error!("{e}");
            json_error(500, "Something went wrong.")
        }
        ServiceError::Mail(e) => {
            console_error!("{e}");
            json_error(
                502,
                "We could not send the email right now. Please try again shortly.",
            )
        }
    }
}

/// Runs Turnstile verification if it is configured. `Err` is a response.
async fn check_turnstile(
    app: &App,
    token: Option<&str>,
    ip: Option<&str>,
) -> std::result::Result<(), Result<Response>> {
    let Some(secret) = &app.turnstile_secret else {
        return Ok(());
    };
    match turnstile::verify(secret, token.unwrap_or(""), ip).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(json_error(
            400,
            "Bot check failed. Please reload and try again.",
        )),
        Err(e) => {
            console_error!("turnstile: {e}");
            Err(json_error(
                502,
                "Bot check is unavailable. Please try again.",
            ))
        }
    }
}

// ------------------------------------------------------------------- public

#[derive(Deserialize)]
struct SubscribeBody {
    email: String,
    #[serde(default)]
    source: String,
    /// Honeypot: real visitors never see or fill this field.
    #[serde(default)]
    website: Option<String>,
    #[serde(default)]
    turnstile_token: Option<String>,
}

async fn subscribe(app: &App, req: Request) -> Result<Response> {
    let ip = client_ip(&req);
    let body: SubscribeBody = match read_json(req, MAX_PUBLIC_BODY).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    // The same answer however the request is handled, so the endpoint cannot
    // be used to learn who is subscribed.
    let generic = || {
        json_response(
            200,
            json!({"ok": true, "message": "Check your inbox to confirm your subscription."}),
        )
    };
    if honeypot_tripped(body.website.as_deref()) {
        return generic();
    }
    if let Err(resp) = check_turnstile(app, body.turnstile_token.as_deref(), ip.as_deref()).await {
        return resp;
    }
    let source = if body.source.trim().is_empty() {
        "website".to_string()
    } else {
        body.source.chars().take(50).collect()
    };
    match app
        .svc()
        .subscribe(&body.email, ip.as_deref(), &source)
        .await
    {
        Ok(
            SubscribeOutcome::ConfirmationSent
            | SubscribeOutcome::RecentlySent
            | SubscribeOutcome::AlreadySubscribed,
        ) => generic(),
        Err(e) => service_error(e),
    }
}

#[derive(Deserialize)]
struct ContactBody {
    name: String,
    email: String,
    #[serde(default)]
    company: String,
    message: String,
    #[serde(default)]
    website: Option<String>,
    #[serde(default)]
    turnstile_token: Option<String>,
}

async fn contact(app: &App, req: Request) -> Result<Response> {
    let ip = client_ip(&req);
    let body: ContactBody = match read_json(req, MAX_PUBLIC_BODY).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let ok = || {
        json_response(
            200,
            json!({"ok": true, "message": "Thanks, your message was sent."}),
        )
    };
    if honeypot_tripped(body.website.as_deref()) {
        return ok();
    }
    if let Err(resp) = check_turnstile(app, body.turnstile_token.as_deref(), ip.as_deref()).await {
        return resp;
    }
    let input = ContactInput {
        name: body.name,
        email: body.email,
        company: body.company,
        message: body.message,
    };
    match app.svc().contact(&input, ip.as_deref()).await {
        Ok(outcome) => {
            if !outcome.notified {
                console_error!("contact message saved but the notification e-mail failed");
            }
            ok()
        }
        Err(e) => service_error(e),
    }
}

fn token_param(url: &Url) -> String {
    url.query_pairs()
        .find(|(k, _)| k == "t")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

fn site_redirect(app: &App, status: &str) -> Result<Response> {
    let target = Url::parse(&format!(
        "{}/newsletter?status={status}",
        app.config.site_url
    ))?;
    Response::redirect(target)
}

async fn confirm(app: &App, url: &Url) -> Result<Response> {
    let token = token_param(url);
    match app.svc().confirm(&token).await {
        Ok(ConfirmOutcome::Confirmed) => site_redirect(app, "confirmed"),
        Ok(ConfirmOutcome::AlreadyConfirmed) => site_redirect(app, "already"),
        Ok(ConfirmOutcome::Expired) => site_redirect(app, "expired"),
        Ok(ConfirmOutcome::Invalid) => site_redirect(app, "invalid"),
        Err(e) => {
            console_error!("confirm: {e}");
            site_redirect(app, "error")
        }
    }
}

/// Showing the page never unsubscribes anyone: mail scanners and link
/// previews fetch links with GET. Only the POST below does it.
fn unsubscribe_page(url: &Url) -> Result<Response> {
    let token = token_param(url);
    html(pages::confirm_unsubscribe(&token))
}

async fn unsubscribe(app: &App, url: &Url) -> Result<Response> {
    let token = token_param(url);
    match app.svc().unsubscribe(&token).await {
        Ok(UnsubscribeOutcome::Done) => html(pages::unsubscribed(
            &app.config.brand_name,
            &app.config.site_url,
        )),
        Ok(UnsubscribeOutcome::Invalid) => {
            Ok(html(pages::invalid_unsubscribe(&app.config.site_url))?.with_status(400))
        }
        Err(e) => {
            console_error!("unsubscribe: {e}");
            json_error(500, "Something went wrong.")
        }
    }
}

fn html(body: String) -> Result<Response> {
    let mut resp = Response::from_html(body)?;
    let h = resp.headers_mut();
    h.set("Cache-Control", "no-store")?;
    h.set("Referrer-Policy", "no-referrer")?;
    h.set("X-Robots-Tag", "noindex")?;
    Ok(resp)
}

// -------------------------------------------------------------------- admin

#[derive(Deserialize)]
struct IssueBody {
    subject: String,
    text: String,
    #[serde(default)]
    html: Option<String>,
}

async fn admin(app: &App, method: Method, path: &str, url: &Url, req: Request) -> Result<Response> {
    let header = req.headers().get("Authorization")?;
    if !bearer_matches(header.as_deref(), &app.admin_token) {
        return json_error(401, "Unauthorized.");
    }
    let segments: Vec<&str> = path.trim_start_matches("/admin/").split('/').collect();
    match (method, segments.as_slice()) {
        (Method::Get, ["stats"]) => match app.svc().stats().await {
            Ok(s) => json_response(200, json!({"ok": true, "stats": s})),
            Err(e) => service_error(e.into()),
        },
        (Method::Post, ["issues"]) => {
            let body: IssueBody = match read_json(req, MAX_ADMIN_BODY).await {
                Ok(b) => b,
                Err(resp) => return resp,
            };
            match app
                .svc()
                .create_issue(&body.subject, &body.text, body.html.as_deref())
                .await
            {
                Ok((issue, recipients)) => json_response(
                    201,
                    json!({"ok": true, "id": issue.id, "recipients": recipients}),
                ),
                Err(e) => service_error(e),
            }
        }
        (Method::Get, ["issues", id]) => match app.svc().issue_progress(id).await {
            Ok(Some((issue, p))) => json_response(
                200,
                json!({"ok": true, "id": issue.id, "subject": issue.subject,
                       "pending": p.pending, "sent": p.sent, "failed": p.failed}),
            ),
            Ok(None) => json_error(404, "No such issue."),
            Err(e) => service_error(e.into()),
        },
        (Method::Post, ["issues", id, "send"]) => {
            match app.store_has_issue(id).await {
                Ok(true) => {}
                Ok(false) => return json_error(404, "No such issue."),
                Err(e) => return service_error(e.into()),
            }
            let batches = url
                .query_pairs()
                .find(|(k, _)| k == "batches")
                .and_then(|(_, v)| v.parse::<u32>().ok())
                .unwrap_or(1)
                .clamp(1, MAX_ADMIN_BATCHES);
            match app.svc().send_batches(Some(id), batches).await {
                Ok(r) => json_response(200, json!({"ok": true, "report": r})),
                Err(e) => service_error(e),
            }
        }
        _ => json_error(404, "Not found."),
    }
}

impl App {
    async fn store_has_issue(
        &self,
        id: &str,
    ) -> std::result::Result<bool, newsify_core::error::StoreError> {
        use newsify_core::store::Store;
        Ok(self.store.find_issue(id).await?.is_some())
    }
}
