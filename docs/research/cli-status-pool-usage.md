# `pengepul status` — pool detail and token stats

## Question

What is the CLI missing ("miggins") if we want `pengepul status` to show the
detail of the pool and token stats — and can we do it?

## Answer

Yes, cheaply. The relay already tracks everything needed and already serves it
on `GET /admin/accounts`; only the CLI rendering is thin. A `status` upgrade is
a CLI-side change with no new route, no protocol change and no persistence
work — unless we also want the numbers to survive a restart, which is the one
real gap.

## What exists today

### The data is already collected per account

`AccountState` (`src/accounts.rs:53-74`) accumulates, in memory:

- `total_requests`, `total_successes`, `total_failures`
- `total_input_tokens`, `total_output_tokens`
- `total_cache_creation_input_tokens`, `total_cache_read_input_tokens`
- `total_reasoning_output_tokens`

Fed by `record_success` / failure paths (`src/accounts.rs:253-269`), which get
a `UsageData` extracted from every response — `usage_from_response`
(`src/app.rs:1719-1762`, accepts both Anthropic and OpenAI `usage` shapes) and
`update_stream_usage` for SSE streams (`src/app.rs:1980`).

### The data is already served

`GET /admin/accounts` (`src/app.rs:766-794`) returns, per provider
(anthropic, codex, and each configured generic endpoint):

```json
{"providers": {"<id>": {"account_count": N, "accounts": [snapshot...]}}}
```

where each snapshot (`AccountManager::snapshots`, `src/accounts.rs:314-343`)
carries: `email`, `available`, `cooldownUntil`, `failureCount`, `lastError`,
`lastFailureAt`, `lastSuccessAt`, `lastRefreshAt`, `totalRequests`,
`totalSuccesses`, `totalFailures`, all five token totals, `expiresAt`,
`planType`.

### What the CLI prints — the actual gap

- `pengepul status` (`src/cli.rs:363-388`): config path, url, server health
  string, then `print_account_counts` — **one line per provider: just
  `"anthropic: 2 accounts"`**. No pool state, no tokens.
- `pengepul accounts` (`print_accounts`, `src/cli.rs:649-686`): per account
  `email available|unavailable failures=N plan=…`. It fetches the same rich
  payload but **drops the request totals and all token totals** that are
  already in each snapshot.

So the "missing" part is not collection, not storage, not an API — it is two
rendering functions.

## Constraints and known limits

- **Totals are in-memory.** They survive `pengepul accounts --reload` (a
  reload keeps existing `AccountState` and only replaces the token,
  `src/accounts.rs:157-175`), but a **process restart zeroes them** — nothing
  writes usage to disk (`tokens.rs` persists credentials only).
- **No pool-wide rollup exists anywhere**; the admin payload is per-account
  only. A `status` view must sum across accounts itself.
- **No per-model, per-key or time-windowed stats.** Those belong to the
  `keyed-usage` / `usage-console` specs (`docs/specs/keyed-usage.md`), which
  require a database and are **not implemented** in this codebase (`grep
  database src/` → nothing). Out of scope for a status upgrade.
- `count_tokens` is deliberately not metered upstream (free), consistent with
  the keyed-usage spec's exclusion.

## Proposal (CLI-only, no server change)

1. Extend `status` to render the pool: per provider, one line with
   `available/total` accounts, requests and successes, then summed token
   totals (input, output, cache-read, cache-creation, reasoning).
2. Add per-account token/request columns to `print_accounts` (or a
   `pengepul accounts --verbose`) reusing fields already in the payload.
3. Optionally humanize (e.g. `12.3M`) — presentation only.

Everything needed is behind the existing authenticated
`GET /admin/accounts`, which `status` already calls.

## Follow-on options (bigger, separate work items)

- **Persist totals** so stats survive restarts (write-behind JSON under the
  auth dir, or adopt the keyed-usage database path).
- **Pool-wide `/admin/summary`** if other consumers want the rollup without
  re-summing.
- **Per-model stats** requires new in-memory aggregation on the request path
  (model is known at record time) plus the same persistence question.
