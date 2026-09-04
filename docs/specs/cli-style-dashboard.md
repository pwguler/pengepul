# cli-style-dashboard

## Goal

`pengepul status` and `pengepul accounts` render each pool as a fixed-width
unicode panel — box-drawn, with state glyphs and share-of-pool bars — when
stdout is a TTY; piped, redirected, or `NO_COLOR` output stays exactly
today's plain text.

## Non-goals

- **No styling outside the two pool commands.** `login`, `serve`, `update`,
  `help` and error output stay plain stderr/stdout strings; no spinner, no
  colored help.
- **No new dependency.** Box-drawing, ANSI codes and glyph rendering are
  hand-rolled in `cli.rs`; no crates.io style crate is added.
- **No terminal-width queries.** Panels are a fixed 60 columns; the layout is
  identical in every terminal.
- **No quota bars.** The bar shows share-of-pool (an account's total tokens
  divided by its pool's total); it never implies a plan limit, which the
  domain does not have.
- **No layout change to plain output.** The 42 existing `tests/cli.rs`
  assertions on plain text keep passing byte-for-byte; the panel renderer is
  additive.
- **No server change.** `GET /admin/accounts` is untouched, as in the
  cli-status-pool-usage spec.

## Acceptance criteria

- AC-1: A `Style` decision enters `run_with_env` as a parameter: `Rich` when
  stdout is a TTY and `NO_COLOR`/`TERM=dumb` are unset, `Plain` otherwise.
  `RealRuntime` answers the TTY question at the edge; the CLI core only sees
  the value. Existing tests keep calling `run_with_env` without breaking
  (a default `Plain` path exists for them).
- AC-2: With `Plain`, `status` and `accounts` print byte-identical output to
  the pre-change build; every pre-existing `tests/cli.rs` case passes
  unchanged.
- AC-3: With `Rich`, `status` prints one panel per provider: a top rule
  `┌─ pool: <id> ─ <N accounts, A available> ─┐`, one row per account with
  email, glyph, state, ok-count, share bar and percentage, a footer section
  with the summed requests line and tokens line, and a bottom rule `└…┘`.
  Panels are a fixed 64 columns — 60 cannot hold the row columns without
  truncating most emails; 64 is still fixed and width-query-free.
  Empty pools render as a single-line panel note, not a broken box.
- AC-4: The share bar is exactly 10 cells: `█` for whole tenths of the
  account's `totalInputTokens + totalOutputTokens + totalCacheRead* +
  totalCacheCreation*` share of the pool total, `░` for the rest, with the
  integer percentage right-aligned. A pool whose total is 0 renders ten `░`
  cells and no percentage.
- AC-5: Account rows carry `● available` (green) for available accounts,
  `● cooldown 4m12s` (amber, remaining time from `cooldownUntil` via the
  existing pure helper) otherwise, and `● unavailable` (red) for an
  unavailable account with no future cooldown — the row drops the plain
  branch's leading "on" because the glyph already states the condition (the
  approved mockup shows `● cooldown`). "on cooldown" stays in plain output.
  Rollup numbers are bold; labels are dim. Color wraps only the glyph/state
  spans, never the whole line.
- AC-6: With `Rich`, `accounts` renders the same panels as `status` and adds
  beneath each account row a dim detail line with in, out, read/write
  totals, plus a second `reasoning` line only when that total is non-zero
  (five fields cannot fit one fixed-width row); the plain branch keeps
  today's detail lines unchanged.
- AC-7: Panel rendering is a pure function from the admin payload and the
  style to strings: no clock reads, no TTY queries, no I/O inside the
  renderer; unit tests pin panel layout, bar math and glyph colors with
  fixed inputs.
- AC-8: `NO_COLOR=1` or `TERM=dumb` on a TTY produces the plain output of
  AC-2, not an uncolored panel — plain is the only non-Rich mode.

## Verification

```sh
cargo test --test cli
cargo test --lib cli
cargo clippy --all-targets -- -D warnings
cargo fmt --check
./target/debug/pengepul status          # TTY: panels
./target/debug/pengepul status | cat    # piped: plain
NO_COLOR=1 ./target/debug/pengepul status   # forced plain
```
