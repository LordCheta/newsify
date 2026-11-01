#!/usr/bin/env bash
# End-to-end smoke test against a local `wrangler dev` with a throw-away D1
# database. E-mail goes to the Worker log (EMAIL_MODE=log), so nothing is sent.
#
#   npm ci && bash scripts/smoke.sh
set -euo pipefail
cd "$(dirname "$0")/.."

PORT="${PORT:-$(python3 -c 'import socket;s=socket.socket();s.bind(("",0));print(s.getsockname()[1])')}"
BASE="http://localhost:$PORT"
STATE="$(mktemp -d)"
LOG="$STATE/dev.log"
ADMIN="Authorization: Bearer $(grep '^ADMIN_TOKEN=' .dev.vars | cut -d= -f2-)"
SITE="http://localhost:5173"

[ -f .dev.vars ] || cp .dev.vars.example .dev.vars

cleanup() { kill "$PID" 2>/dev/null || true; wait "$PID" 2>/dev/null || true; rm -rf "$STATE"; }

npx wrangler d1 migrations apply newsify --local --persist-to "$STATE" >/dev/null
npx wrangler dev --local --persist-to "$STATE" --port "$PORT" \
  --var EMAIL_MODE:log --var "PUBLIC_BASE_URL:$BASE" --var "SITE_URL:$SITE" \
  --var "ALLOWED_ORIGINS:$SITE" >"$LOG" 2>&1 &
PID=$!
trap cleanup EXIT

for _ in $(seq 1 180); do
  curl -fs "$BASE/health" >/dev/null 2>&1 && break
  kill -0 "$PID" 2>/dev/null || break
  sleep 1
done
curl -fs "$BASE/health" >/dev/null || { echo "worker did not start"; tail -30 "$LOG"; exit 1; }

fail() { echo "FAIL: $*"; tail -30 "$LOG"; exit 1; }
step() { echo "== $*"; }
status() { curl -s -o /dev/null -w '%{http_code}' "$@"; }

step "subscribe (generic answer, confirmation e-mail logged)"
out=$(curl -s -X POST "$BASE/api/subscribe" -H 'Content-Type: application/json' \
  -H "Origin: $SITE" -d '{"email":"Reader@Example.com"}')
echo "$out" | grep -q '"ok":true' || fail "subscribe: $out"
sleep 1
link=$(grep -o "$BASE/confirm?t=[0-9a-f]*" "$LOG" | tail -1)
[ -n "$link" ] || fail "no confirmation link in log"

step "subscribing again looks identical (no enumeration)"
out2=$(curl -s -X POST "$BASE/api/subscribe" -H 'Content-Type: application/json' -d '{"email":"reader@example.com"}')
[ "$out" = "$out2" ] || fail "responses differ: $out vs $out2"

step "not mailed before confirming"
r=$(curl -s -X POST "$BASE/admin/issues" -H "$ADMIN" -H 'Content-Type: application/json' \
  -d '{"subject":"Issue 0","text":"nobody should get this"}')
echo "$r" | grep -q '"recipients":0' || fail "expected 0 recipients before confirm: $r"

step "confirm redirects to the site"
loc=$(curl -s -o /dev/null -w '%{redirect_url}' "$link")
[ "$loc" = "$SITE/newsletter?status=confirmed" ] || fail "redirect was $loc"
loc=$(curl -s -o /dev/null -w '%{redirect_url}' "$link")
[ "$loc" = "$SITE/newsletter?status=already" ] || fail "second confirm redirect was $loc"

step "send an issue"
r=$(curl -s -X POST "$BASE/admin/issues" -H "$ADMIN" -H 'Content-Type: application/json' \
  -d '{"subject":"Hello","text":"First issue\n\nThanks for reading."}')
echo "$r" | grep -q '"recipients":1' || fail "expected 1 recipient: $r"
id=$(echo "$r" | sed -E 's/.*"id":"([0-9a-f]+)".*/\1/')
curl -s -X POST "$BASE/admin/issues/$id/send" -H "$ADMIN" | grep -q '"sent":1' || fail "send failed"
curl -s "$BASE/admin/issues/$id" -H "$ADMIN" | grep -q '"sent":1' || fail "progress wrong"
grep -q "to=reader@example.com subject=\"Hello\"" "$LOG" || fail "issue e-mail not logged"
unsub=$(grep -o "$BASE/unsubscribe?t=[0-9a-f.]*" "$LOG" | tail -1)
[ -n "$unsub" ] || fail "no unsubscribe link in the issue"

step "unsubscribe: GET only shows a page, POST does it"
curl -s "$unsub" | grep -q 'Yes, unsubscribe me' || fail "no confirm page"
curl -s "$BASE/admin/stats" -H "$ADMIN" | grep -q '"active":1' || fail "GET must not unsubscribe"
code=$(status -X POST "$unsub" -d 'List-Unsubscribe=One-Click')
[ "$code" = 200 ] || fail "unsubscribe POST returned $code"
curl -s "$BASE/admin/stats" -H "$ADMIN" | grep -q '"unsubscribed":1' || fail "not unsubscribed"

step "tampered unsubscribe link is rejected"
code=$(status -X POST "${unsub%?}g")
[ "$code" = 400 ] || fail "tampered link returned $code"

step "contact form"
r=$(curl -s -X POST "$BASE/api/contact" -H 'Content-Type: application/json' \
  -d '{"name":"Ada","email":"ada@example.com","message":"Hello, I would like to talk."}')
echo "$r" | grep -q '"ok":true' || fail "contact: $r"
grep -q "ada@example.com" "$LOG" || fail "contact notification not logged"

step "validation, auth and CORS"
[ "$(status -X POST "$BASE/api/subscribe" -H 'Content-Type: application/json' -d '{"email":"nope"}')" = 400 ] || fail "bad email accepted"
[ "$(status "$BASE/admin/stats")" = 401 ] || fail "admin open without token"
[ "$(status -X OPTIONS "$BASE/api/subscribe" -H 'Origin: https://evil.example' -H 'Access-Control-Request-Method: POST')" = 403 ] || fail "CORS preflight allowed a stranger"
[ "$(status -X OPTIONS "$BASE/api/subscribe" -H "Origin: $SITE" -H 'Access-Control-Request-Method: POST')" = 204 ] || fail "CORS preflight denied the site"

echo "All smoke checks passed."
