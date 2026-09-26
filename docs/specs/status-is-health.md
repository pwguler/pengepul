# status-is-health

## Goal

`pengepul status` says whether the relay can serve, and `pengepul usage` says how much it has
carried: each figure has one home. Before this, both verbs printed the same relay total block,
and `usage` also printed `all time` beside it.

The operator's words: *"pengepul status and usage seems overlap"*, then *"keep only one box"*,
and *"peak (all time) and all should be all time as well except last 30 days"*.

## Shape

**`status`** is one box headed `relay`: `config`, `url`, `server`,
`version`, `uptime`, then one row per Pool naming what it can serve:
`2 accounts, 1 available, 1 on cooldown`. `available` always prints; `on cooldown`, `disabled`
and `unavailable` print only when non-zero, in the words the account rows use. No request or
token figure.

**`usage`** is one box headed `usage ─ P pools, A accounts`. The first two rows cover the last
30 days; every row after them is all-time:

```
tokens        <sparkline>
last 30 days  9.5B
peak          1.3B  2026-09-23
total         11.0B
requests      62,503  (60,164 ok, 2,339 failed)
input / cached / uncached / output / reasoning
```

`total` is `input + output`, a labelled total above its own breakdown (ADR-0024).

**The all-time peak** is the relay's best local day, every account of every Pool summed. Daily
buckets are kept 90 days, so the relay persists its best day in `<auth-dir>/usage-peak.json`
and reports it in the admin payload as `peak: {date, tokens}`. The stored day is replaced
whenever the retained buckets hold a better one, at startup and each time the payload is built.
A day stays the peak on record only if the relay starts or the payload is built at least once
while that day is still inside the 90-day retention. Only a missing file means no peak: an
unreadable one is left in place and never overwritten.

## Non-goals

- **No change to `accounts`.**
- **No change to what is recorded:** counters, daily buckets, their 90-day retention and
  `usage.json` stay as they are. The peak is a new file beside them, not a field in them.
- **No per-Pool peak,** and no peak in `status`.
- **The plain daily rows of `usage` keep their shape** (`<date> <requests> …`), unlabelled.
- **No backfill of a peak older than the retained buckets.**

## Acceptance criteria

- AC-1: `status` prints no `requests` row and no token row, in either style.
- AC-2: `status` rich is one box headed `relay`; plain's first line is `relay`. The pool rows
  carry each pool and its account count, so the header names only the subject.
- AC-3: Each non-empty Pool has one `status` row, which rich wraps between counts onto an
  unlabelled continuation line when it is wider than the value cell: `N account(s), N available`, then
  `, N on cooldown`, `, N disabled`, `, N unavailable` for each non-zero count. The counts
  partition the Pool's accounts. Rich paints the available count green, or red at zero,
  `on cooldown` amber and `disabled` dim. Empty Pools print no row.
- AC-4: `usage` rich is exactly one box, headed `usage ─ P pools, A accounts`, with the rows
  `tokens`, `last 30 days`, `peak`, `total`, `requests`, `input`, `cached`, `uncached`,
  `output`, then `reasoning` when non-zero, in that order, on one label column. No `window` or
  `all time` row and no second box.
- AC-5: `usage` plain prints `usage: P pools, A accounts`, the daily rows, then
  `last_30_days <n>`, `peak <n> <date>`, `total <n>`, `requests …` and the token line.
- AC-6: `total` equals `input + output` as printed by the token block's source counters, and
  is never less than `last 30 days`.
- AC-7: `peak` is the greatest of the payload's `peak` and every retained day in the payload,
  inside the 30-day window or not. With neither, the row is absent.
- AC-8: The relay writes `<auth-dir>/usage-peak.json` when its retained buckets hold a day
  above the stored one, never lowers it, and reports it as the payload's `peak`. A stored peak
  whose day is no longer retained is still reported.
- AC-9: A relay with no recorded day prints `tokens  no usage recorded yet` and zero figures in
  the one box, and no `peak` row. A relay whose history is all older than the window prints
  `tokens  none in the last 30 days` above its all-time rows.

## Verification

```
cargo test --locked status
cargo test --locked usage
cargo test --locked peak
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
pengepul status; pengepul usage   # against a relay started from this build
```
