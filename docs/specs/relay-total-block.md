# relay-total-block

## Goal

`pengepul status` ends with one aggregate block, after all pool panels:
the **Relay total** — two headline numbers across every pool of the
relay: **total requests** and **total tokens**, where total tokens =
in + out + cache-read + cache-write (the same sum that drives the share
bars).

## Non-goals

- **No request breakdown in the block** (no ok/failed split, no in/out/
  read/write split): the user asked for the two totals; the per-pool
  panels above already carry the breakdowns.
- **No per-key or per-pool TOTAL columns.** The per-account detail lines
  and pool footers keep their current fields; the totals exist only at
  relay level.
- **`accounts` output unchanged.** Both plain and rich branches keep
  today's shape.
- **No reasoning in total tokens.** Reasoning stays where it is today
  (per-account line, per-pool conditional line); the token total matches
  the share-bar sum exactly.
- **No persistence.** Same in-memory lifetime as all other stats; resets
  on restart.
- **No server change.** The block sums the existing `GET /admin/accounts`
  payload client-side.

## Acceptance criteria

- AC-1: With `Style::Plain`, `status` prints, after the last pool line, a
  blank line, then `relay total: P pools, A accounts`, then
  `total requests N`, then `total tokens T` — where P, A count the
  provider entries and their loaded accounts in the admin payload, N is
  the sum of `totalRequests` over all accounts, and T is the sum over all
  accounts of input + output + cache-read + cache-creation tokens.
  Existing plain-output lines keep passing unchanged.
- AC-2: With `Style::Rich`, the same three lines print after the last
  panel, preceded by a rule line `──── relay total: P pools, A accounts
  ────` whose visible width is 64; every block line is at most 64 visible
  columns, and the rule is exactly 64.
- AC-3: The numbers are the relay-wide sums: a fixture with two providers
  (one account 640 requests / in 33, out 120; one account 10 requests /
  in 10, out 5) yields `total requests 650` and `total tokens 168`;
  adding a third pool with 0-token, 0-request accounts keeps both.
- AC-4: An empty relay (all pools with zero accounts) still prints the
  block: `relay total: 2 pools, 0 accounts`, `total requests 0`,
  `total tokens 0`.
- AC-5: Counts include empty pools: `account_count` entries with no
  accounts still add to P, and each loaded account adds to A.
- AC-6: The block reads no clock and performs no I/O — it is computed
  from the same in-memory payload the panels use, inside the existing
  renderer call.

## Verification

```sh
cargo test --test cli
cargo test --lib cli
cargo clippy --all-targets -- -D warnings
cargo fmt --check
script -qec "./target/debug/pengepul status" /dev/null   # block visible on tty
./target/debug/pengepul status | tail -3                 # block in plain
```
