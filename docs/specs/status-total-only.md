# status-total-only

## Goal

`pengepul status` stops printing per-pool panels. It prints exactly one
block — the relay total — carrying: the connection lines, one summary
line per pool, and the relay-wide request/token aggregate. Per-pool and
per-account detail moves to a single home: `pengepul accounts`.

The user's words: *"for pengepul status no need to show each pool, only
the total"*, then, choosing the shape: total **plus one line per pool**.

## Non-goals

- **`accounts` keeps every panel.** It stays the detailed view (and gains
  per-model lines under `usage-by-model`); nothing is removed there.
- **No new flag.** No `status --pools` escape hatch; `accounts` is the
  answer to "show me the detail". Fewer surfaces, one home per view.
- **No data change.** Same `GET /admin/accounts` payload, same
  client-side sums; only the renderer changes.
- **No change to `accounts`, `service`, `login`, `update`, `config`.**
- **Empty pools stay hidden** in `status` (they have no accounts and
  nothing to summarize). `accounts` keeps listing them in its plain
  branch (`codex: 0 accounts`); its rich branch has skipped them since
  v0.10.2, which this spec does not change either way.

## Acceptance criteria

- AC-1: With `Style::Plain`, `status` prints **no pool panel and no
  per-account line**. Its whole output is the relay total block:
  `relay total: P pools, A accounts`, `config <path>`,
  `url <url> — server <state>`, one line per non-empty pool, then
  `requests N  (S ok, F failed)`, `tokens in X  out Y  cache Z`,
  optional `reasoning R`, and `total T`.
- AC-2: With `Style::Rich`, the same content renders as **one 64-column
  box panel** headed `relay total ─ P pools, A accounts` — replacing
  today's bare rule. Every line is at most 64 visible columns and the box
  borders are exactly 64.
- AC-3: The per-pool line reads `<pool>  <n> account(s)  <r> req  <t>` —
  pool name, account count (singular/plural), request count, and total
  tokens for that pool (in + out + cache-read + cache-creation), with the
  token figure right-aligned. Pools appear in payload order.
- AC-4: Pools with zero accounts are omitted from the per-pool lines and
  from `P`, matching today's status behaviour; `A` counts loaded
  accounts.
- AC-5: The aggregate lines are relay-wide sums over all accounts:
  requests, successes, failures, input, output, cache (read + creation),
  reasoning, and `total` = in + out + cache. `reasoning` prints only when
  non-zero.
- AC-6: An empty relay (no pools with accounts) still prints the block
  with `relay total: 0 pools, 0 accounts`, no per-pool lines, and zeroed
  aggregates.
- AC-7: **Superseded.** This spec does not change `accounts`, but the
  paired `usage-by-model` spec deliberately does, and both land in the
  same range — so "byte-identical `accounts` output" is not a property
  the tree can be judged against, as the landing judge showed. What is
  verified instead: this spec's own commit leaves `accounts` untouched,
  and the plain `accounts` assertions in the suite pass unedited through
  it.
- AC-8: The renderer stays pure: no clock read beyond the `now` already
  passed in, no I/O.

## Revisions

- The two chosen previews disagreed on the aggregate: the content option
  showed `requests` + `total` only, the style option showed the
  `tokens in/out/cache` breakdown. **Merged: both.** Dropping the
  breakdown would lose information that today's pool footers carry, and
  the complaint was about the bulk of the panels, not the aggregate. The
  block is 8–9 lines against today's 23.
- Header keeps the existing `relay_header()` wording (`relay total: P
  pools, A accounts`) so plain and rich share one string; the rich box
  renders it as its panel header.

## Verification

```sh
cargo test --test cli
cargo test --lib usage_view
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Manual: `pengepul status` piped and under `script -qec`, plus
`pengepul accounts` in both styles diffed against the v0.10.2 binary.
