# Enhancement: /api/v1/health should reflect database readability

**Requested by:** mkube (2026-08-10)
**Context:** mkube v6.2.1 replaced its DNS-client failure blacklist with an
alive gate on `GET /api/v1/health` — before touching an endpoint, mkube probes
health (15s TTL cache, 1.5s timeout) and skips all DNS operations against an
instance whose probe fails. That makes the health endpoint the single signal
mkube uses to decide "is this microdns usable right now."

## Problem

Today `/api/v1/health` returning 200 confirms the HTTP listener is up, but not
that the instance can actually serve or accept records. Since microdns is
database-driven, an instance whose database is missing, locked, or corrupt can
still answer 200 — mkube would then proceed and fail on every real operation.

## Proposed behavior

`/api/v1/health` returns 200 only when a trivial database read succeeds
(e.g. `SELECT count(*) FROM zones` or equivalent). Otherwise return 503 with a
short JSON body naming the failing check.

- The check must be cheap (single indexed read) — mkube probes at most once
  per 15s per instance, but other consumers may poll more often.
- "Database readable" is the right bar, **not** "database non-empty" — a
  freshly bootstrapped instance with zero zones is healthy and must return 200.
- Optionally include counts in the response body (`{"zones": N, "records": M}`)
  for observability; mkube only looks at the status code.

## Consumer contract (mkube side, already shipped)

- 200 → instance treated as alive; DNS register/clean/list operations proceed.
- Anything else (non-200, timeout >1.5s, connection error) → instance treated
  as down; mkube skips all DNS ops against it for up to 15s, then re-probes.
