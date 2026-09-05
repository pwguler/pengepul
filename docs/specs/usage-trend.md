# usage-trend

## Goal

`pengepul usage` — a four-line panel showing the last 30 days of token
traffic as a sparkline, so the operator can see the shape of their usage
over time rather than one cumulative number.

The user's words: *"gua mau tau trend per hari"*, then, choosing the
shape from three prototypes: sparkline only, four lines.

## The shape

```
┌─ usage ─ last 30 days ───────────────────────────────────────┐
│ tokens   ▁▁▁▁▁▁▁▁▂▂▁▁▁▂▂▃▁▁▁▂▃▄▄▁▁▂▃▅▆█                      │
│ peak     120.6M on 09-06                                     │
│ total    563.3M across 30 days                               │
└──────────────────────────────────────────────────────────────┘
```

A bar is one **local** calendar day (Asia/Jakarta on this host). The
operator works 21:00–01:00 WIB; a UTC bucket would cut every evening
session across two bars, because their day begins at 17:00 UTC the day
before.

## Non-goals

- **Not the per-harness breakdown.** The user queued that explicitly
  ("trend first"). Nothing here records a client identity, and the
  bucket shape must not make adding one later require a migration.
- **No database, no console, no workspace split.** `docs/specs/`
  contains `workspace-split`, `keyed-usage`, `usage-console` and
  `web-console` — a settled but unbuilt design answering this ask with
  Postgres, Prisma and Next.js. This spec deliberately does not reopen
  it: a relay reading its own traffic does not need a product.
- **No per-model daily buckets.** Per-account per-day only. Per-model
  multiplies the file by the model count for a question not asked; the
  cumulative per-model view already exists in `accounts`.
- **No backfill.** The history does not exist — `usage.json` holds
  cumulative counters only, and the journal has eight days of lifecycle
  lines. Day one shows one bar. This is stated in the panel when the
  window is not yet full.
- **No change to `status` or `accounts`.** Both keep their current
  shape and their plain bytes.
- **No new dependency.** `chrono` already carries the `clock` feature,
  so `Local` is available.

## Acceptance criteria

- AC-1: `AccountManager::record_success` / `record_attempt` /
  `record_failure` add their outcome to a bucket keyed by the **local**
  calendar date (`%Y-%m-%d` in the host's timezone) in addition to the
  cumulative counters, for the account they already touch.
- AC-2: A daily bucket holds the same eight counters as the cumulative
  record: requests, successes, failures, input, output, cache-creation,
  cache-read, reasoning. Tokens are what the sparkline renders; the rest
  are stored so a later view needs no backfill.
- AC-3: Buckets persist in `usage.json` under a `days` map per email,
  keyed by date string, written by the same atomic temp+rename with the
  same `0600`/`0700` modes. A file written before this change loads with
  cumulative counters intact and no daily history.
- AC-4: Retention is **90 days**: on write, buckets older than 90 local
  days are dropped. A file that somehow holds more loads fine and is
  trimmed on the next write.
- AC-5: `pengepul usage` (rich) prints one panel: header
  `usage ─ last 30 days`, a `tokens` row carrying a 30-character
  sparkline, a `peak` row naming the largest day and its date, and a
  `total` row summing the window. Exactly 64 columns, in the panel
  grammar `consistent-panels` settled.
- AC-6: The sparkline uses `▁▂▃▄▅▆▇█`, one character per day, oldest
  left. Height scales to the window's peak. A day with zero traffic
  renders `▁`, never a blank — a gap would read as missing data rather
  than an idle day.
- AC-7: `pengepul usage` (plain) prints one line per day,
  `<date> <requests> <input> <output> <cache> <reasoning>`, oldest
  first — parseable, no block characters, no box.
- AC-8: A relay with no daily history prints the panel with an empty
  window and says so rather than rendering a flat line of `▁` that
  would look like 30 idle days. A **partial** window (fewer recorded
  days than the window is wide) names how many days it actually has and
  when daily history began; a full window drops that note.
- AC-11: The numbers reconcile across verbs. `usage` prints `window`
  (this window's tokens) beside `all time` (every token the relay has
  counted), the latter computed by the same `account_tokens` sum that
  `status` prints, over the same payload, so the two can never drift.
  No verb prints a bare `total`: one word, one scope. `all time` is
  never less than the `window` it contains, even when a payload's
  cumulative counters are absent or lag its buckets.
- AC-9: Days are summed across every account of every pool: the
  sparkline is relay-wide, matching what `status` totals. This requires
  `GET /admin/accounts` to carry a `days` array per account, in the same
  camelCase shape as `models` — implied by AC-9 but stated by no
  criterion when this spec was written, and found during implementation.
- AC-10: The renderer stays pure — it takes the buckets and a `now`
  from the command layer, reads no clock and performs no I/O.

## Revisions

- **The payload gap.** AC-9 sums days relay-wide and the CLI reads
  `/admin/accounts`, but no criterion said the payload carries `days`.
  Found while implementing AC-1; folded into AC-9 rather than left
  implicit.
- **`total` was three different scopes.** `status` said `total 353.4M`
  (all time) and `usage` said `total 5.9M` (a 30-day window) with
  nothing on screen relating them, so the operator compared the two and
  reasonably concluded the trend was broken. It was merely new. The word
  `total` is now gone from `usage`: `window` and `all time` name their
  scopes, and `all time` is the figure `status` prints. Found by the
  user asking why the numbers disagreed — AC-11 exists because of it.
- **A subset could exceed its superset.** A payload carrying buckets but
  no cumulative counters printed `all time 0` under `window 11.0K`.
  Clamped: `all time` is at least the window it contains.
- **The sparkline covers calendar days, not recorded days.** A window of
  30 columns needs a column per *calendar* day, so idle days are
  synthesised as zero rather than skipped. Without this the line would
  compress an idle week into nothing and misreport the shape.

## Verification

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Manual: `pengepul usage` under `script -qec` and piped; drive traffic
through the relay and confirm today's bucket moves;
`cat ~/.pengepul/anthropic/usage.json` to see the `days` map.
