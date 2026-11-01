# newsify

A small, self-hosted **newsletter and contact-form API** that runs on
**Cloudflare Workers**. Written in Rust, stores data in **D1**, sends mail
through **Resend**.

It gives a static site (Svelte, Astro, plain HTML, anything) three things
without a server to run or a monthly newsletter bill:

- **Double opt-in subscriptions**: nobody is mailed until they click a link.
- **Sending issues** to confirmed subscribers, in rate-limited batches, with
  one-click unsubscribe.
- **A contact form endpoint** that stores each message and e-mails it to you.

```
 browser (your site)                 Cloudflare                      Resend
 ───────────────────   POST /api/subscribe   ┌──────────────────┐
  <form> / fetch()  ─────────────────────────▶   newsify Worker  │──▶ confirmation e-mail
                        POST /api/contact    │  (Rust → wasm)   │──▶ contact notification
                                             │        │         │
 e-mail link  ───────▶ GET /confirm          │        ▼         │
                       GET|POST /unsubscribe │   D1 (SQLite)    │
 you (curl)   ───────▶ POST /admin/issues    │        ▲         │
                                             │  cron, every min─┼──▶ issue batches (≤ 100 each)
                                             └──────────────────┘
```

## Why this design

- **Pure core, thin adapter.** `crates/newsify-core` holds every rule
  (validation, the subscription state machine, tokens, rate limiting, batching,
  templates). It only talks to the world through three small traits (`Store`,
  `Mailer`, `Platform`), so it is tested natively and fast, with in-memory
  fakes. `crates/newsify-worker` is the Cloudflare adapter: HTTP routing, D1,
  Resend, the cron handler.
- **Safe by default.** See [Security](#security).
- **Cheap.** One Worker, one D1 database. The free plans are enough for a
  personal or small-business site (see [Limits](#limits)).

## API

All JSON endpoints answer `{"ok": true, ...}` or `{"ok": false, "error": "..."}`.
Public endpoints are CORS-restricted to `ALLOWED_ORIGINS`.

| Method and path | Purpose |
| --- | --- |
| `POST /api/subscribe` | Body `{"email", "source"?, "website"?, "turnstile_token"?}`. Starts double opt-in. The response is identical whether or not the address is already known. |
| `GET /confirm?t=TOKEN` | The link in the confirmation e-mail. Redirects to `SITE_URL/newsletter?status=confirmed\|already\|expired\|invalid\|error`. |
| `GET /unsubscribe?t=TOKEN` | Shows a "Yes, unsubscribe me" page. Does **not** unsubscribe (link scanners fetch URLs). |
| `POST /unsubscribe?t=TOKEN` | Unsubscribes. This is also the RFC 8058 one-click endpoint used by mail clients. |
| `POST /api/contact` | Body `{"name", "email", "company"?, "message", "website"?, "turnstile_token"?}`. Stores the message and e-mails it to `CONTACT_TO` with `Reply-To` set to the visitor. |
| `GET /health` | Liveness. |
| `POST /admin/issues` | Bearer `ADMIN_TOKEN`. Body `{"subject", "text", "html"?}`. Creates an issue and queues it for everyone active **right now**. Returns `{"id", "recipients"}`. |
| `POST /admin/issues/:id/send?batches=N` | Bearer. Sends up to N (1 to 5) batches immediately. The cron does this too. |
| `GET /admin/issues/:id` | Bearer. `{"pending", "sent", "failed"}`. |
| `GET /admin/stats` | Bearer. Subscriber, issue and message counts. |

`website` is a honeypot field: real visitors never fill it, bots do. If it is
non-empty the request is accepted and silently dropped.

## Sending a newsletter

```bash
# 1. Create the issue. `text` is required; `html` is optional (trusted, inserted as is).
curl -sX POST "$NEWSIFY/admin/issues" \
  -H "Authorization: Bearer $ADMIN_TOKEN" -H 'Content-Type: application/json' \
  -d '{"subject":"October update","text":"Hello!\n\nHere is what we shipped..."}'
# → {"ok":true,"id":"3f9c…","recipients":42}

# 2. Nothing else to do: the cron trigger sends one batch (default 50) per minute.
#    Watch it, or push it along:
curl -s "$NEWSIFY/admin/issues/3f9c…" -H "Authorization: Bearer $ADMIN_TOKEN"
curl -sX POST "$NEWSIFY/admin/issues/3f9c…/send?batches=5" -H "Authorization: Bearer $ADMIN_TOKEN"
```

Every e-mail gets an unsubscribe footer and `List-Unsubscribe` /
`List-Unsubscribe-Post` headers added automatically. People who unsubscribe
while an issue is going out are skipped.

## Setup

You need a Cloudflare account, a domain verified in [Resend](https://resend.com)
(so mail comes from `you@yourdomain`), Node 20+ and Rust (stable).

```bash
npm install                                # wrangler
rustup target add wasm32-unknown-unknown

# 1. Database
npx wrangler d1 create newsify             # copy the database_id it prints into wrangler.toml
npm run db:migrate:remote

# 2. Settings: edit [vars] in wrangler.toml (BRAND_NAME, PUBLIC_BASE_URL, SITE_URL,
#    ALLOWED_ORIGINS, FROM_EMAIL, CONTACT_TO). EMAIL_MODE stays "resend".
#    FROM_EMAIL ships as a REPLACE_ME placeholder: set a Resend-verified sender first.

# 3. Secrets
openssl rand -hex 32 | npx wrangler secret put SIGNING_SECRET
openssl rand -hex 32 | npx wrangler secret put ADMIN_TOKEN     # note the value: it is your admin password
npx wrangler secret put RESEND_API_KEY
# optional: npx wrangler secret put TURNSTILE_SECRET

# 4. Deploy (builds the Rust Worker, then uploads it with the cron trigger)
npx wrangler deploy
```

Keep `SIGNING_SECRET` stable: changing it invalidates every unsubscribe link
already sitting in people's inboxes.

### Settings reference

| Name | Kind | Meaning |
| --- | --- | --- |
| `BRAND_NAME` | var | Name used in e-mails and pages. |
| `PUBLIC_BASE_URL` | var | Public URL of this Worker; used in e-mail links. Use a custom domain, e.g. `https://api.example.com`, so links look trustworthy. |
| `SITE_URL` | var | Your website. After confirming, people land on `SITE_URL/newsletter?status=...`. |
| `ALLOWED_ORIGINS` | var | Comma-separated origins allowed to call `/api/*` from a browser. |
| `FROM_EMAIL` | var | Sender, e.g. `Name <news@example.com>`. Must be on a Resend-verified domain. |
| `CONTACT_TO` | var | Inbox that receives contact-form messages. |
| `EMAIL_MODE` | var | `resend` (real mail) or `log` (print mail to the Worker log; local dev). |
| `BATCH_SIZE` | var | E-mails per cron tick, 1 to 100. Default 50. |
| `SIGNING_SECRET` | secret | 16+ chars. Signs unsubscribe links and keys the IP hashes. |
| `RESEND_API_KEY` | secret | Resend API key. Not needed in `log` mode. |
| `ADMIN_TOKEN` | secret | Bearer token for `/admin/*`. If unset, the admin API is closed. |
| `TURNSTILE_SECRET` | secret, optional | If set, `/api/subscribe` and `/api/contact` require a valid `turnstile_token`. |

## Local development

```bash
cp .dev.vars.example .dev.vars   # local-only secrets, git-ignored
npm run db:migrate:local
npm run dev                      # http://localhost:8787, e-mail printed to the console
```

In `log` mode confirmation links and issues appear in the terminal, so you can
click through the whole flow without sending anything.

## Tests

```bash
cargo test --workspace     # unit + end-to-end flows against in-memory fakes
bash scripts/smoke.sh      # builds the Worker and drives a real wrangler dev with a fresh local D1
```

`crates/newsify-core/tests/flows.rs` covers the double opt-in lifecycle,
rate limits, enumeration resistance, batching, idempotency keys, retry and
failure handling, unsubscribing mid-send, and the contact form. CI
(`.github/workflows/ci.yml`) runs fmt, clippy (native and wasm), the tests, and
the smoke test on every push and pull request.

## Security

- **Double opt-in.** Signing up only creates a `pending` subscriber. Pending
  and unsubscribed addresses are never mailed an issue.
- **No account enumeration.** `/api/subscribe` answers the same way for new,
  pending and active addresses.
- **Tokens.** Confirmation tokens are 256-bit random values; only their SHA-256
  is stored, so a database leak cannot be used to confirm addresses. Unsubscribe
  links are HMAC-signed, never expire, and carry no personal data.
- **Scanner-safe unsubscribe.** `GET` shows a page; only `POST` unsubscribes.
- **Abuse limits.** Per-IP rate limits (5 signups and 3 contact messages per
  hour) keyed on an HMAC of the IP, never the IP itself; a honeypot; optional
  Turnstile; a 20 KB body limit on public endpoints.
- **Header injection.** Names, subjects and addresses reject control
  characters and line breaks before they can reach a mail header.
- **Admin API.** Bearer token compared in constant time; closed when
  `ADMIN_TOKEN` is unset. Keep the token out of the browser.
- **Failed mail.** If sending the confirmation fails, the visitor's cooldown is
  cleared so they can retry immediately. Contact messages are stored before the
  notification is sent, so none are lost.

## Limits

- **Workers free plan: 50 subrequests per invocation.** One cron tick sends one
  batch of up to 50 (one Resend call plus a few D1 operations), which is well
  inside that. At 50 per minute, 1,000 subscribers take about 20 minutes. On a
  paid plan raise `BATCH_SIZE` (max 100).
- **Resend free plan** has daily and monthly sending caps; check your plan
  before a large issue. A 429 from Resend pauses the queue without using up
  retry attempts.
- Each batch is sent with an idempotency key derived from its contents, so a
  retry after a timeout does not double-send.
- Delivery bounces and complaints are not processed automatically yet.

## Project layout

```
crates/newsify-core/     rules, traits, templates, in-memory fakes, tests
crates/newsify-worker/   Cloudflare adapter: routing, D1, Resend, Turnstile, cron
migrations/              D1 schema
scripts/smoke.sh         end-to-end test against wrangler dev
wrangler.toml            bindings, vars, cron
```

## License

MIT, see [LICENSE](LICENSE).
