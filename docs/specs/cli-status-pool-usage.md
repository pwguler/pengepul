# cli-status-pool-usage

## Goal

`pengepul status` shows the pool: per-provider rollups of account availability
(with cooldown detail), request outcomes and token totals; `pengepul accounts`
gains per-account request and token detail. Both read the existing
`GET /admin/accounts` payload; the server is untouched.

## Non-goals

- **No server change.** No new route, no new field in any snapshot, no
  `/admin/summary`. The payload already carries everything; the diff is CLI
  rendering only.
- **No persistence.** Totals stay in memory; a relay restart zeroes them, and
  the README says so ("stats reset on restart"). A future usage-state file
  under the auth dir is a separate work item.
- **No per-model, per-key or time-windowed stats.** Those belong to the
  keyed-usage / usage-console specs, which need a database and are not
  implemented here.
- **No change to existing output strings.** `status` keeps its config/url/
  server lines; `accounts` keeps `email state failures=N plan=…` and appends
  detail rather than rewording it.
- **No machine-readable output mode** (JSON / `--format`): presentation only;
  scripts can use `/admin/accounts` directly.

## Acceptance criteria

- AC-1: `status` prints, per provider under `providers` in the admin payload,
  a blank line then a rollup block: an availability header
  `<id>: N accounts (A available)` — appending `, K on cooldown 4m12s` (per
  account, comma-separated, remaining time) when any account's `cooldownUntil`
  is in the future — then a `requests X (Y ok, Z failed)` line and a
  `tokens in … out … cache-read … cache-write … reasoning …` line, where the
  tokens are the sum of that provider's account snapshots and all five token
  fields always print, including zero.
- AC-2: The cooldown suffix derives from `cooldownUntil` against the wall
  clock at print time and rounds down to whole minutes and seconds; a helper
  unit test proves the rendering for a fixed instant (no `CliRuntime`).
- AC-3: Numbers print humanized: thousands separators for values under one
  million, and `K`/`M` suffixes at one decimal above that (e.g. `1,204`,
  `812.3K`, `45.2M`); a helper unit test pins the table.
- AC-4: With a provider whose accounts are all available and totals all zero,
  the rollup prints `requests 0  (0 ok, 0 failed)` and all five token fields
  as `0`.
- AC-5: `accounts` prints, under each existing account line, an indented
  detail line `requests X (Y ok) in A out B cache-read C cache-write D` plus
  `reasoning E` when the snapshot's reasoning total is non-zero; the detail
  omits `plan=`/state duplication and reuses the same humanizer.
- AC-6: `accounts` renders an account with `available == false` as
  `on cooldown 4m12s` (remaining time derived from `cooldownUntil`) instead
  of `unavailable`; the account header format is otherwise unchanged. This
  follows CONTEXT.md, whose **Cooldown** entry avoids "unavailable" and
  "cooling down".
- AC-7: A provider entry with `account_count: 0` and an empty `accounts`
  array prints only its `N accounts` header line, in both commands.
- AC-8: All pre-existing `tests/cli.rs` cases pass unchanged, proving the
  legacy `status` lines (config/url/server + `N accounts`) and legacy
  `accounts` lines are byte-identical.

## Verification

```sh
cargo test --test cli
cargo test --lib cli
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```
