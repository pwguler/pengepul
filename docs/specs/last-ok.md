# last-ok

## Goal

`pengepul accounts` says when each account last served a request successfully, and the answer
survives a restart.

The payload already carried `lastSuccessAt`, but nothing printed it, a restart reset it to
null, and a successful token Refresh wrote it too, so it could call an account healthy that had
served nothing in days.

## Non-goals

- **No new payload field.** `lastSuccessAt` keeps its name and its ISO-8601 format; only what
  writes it changes.
- **No change to what a Refresh does otherwise.** It still clears the Cooldown, the failure
  streak and the last error, and still writes `lastRefreshAt`.
- **No change to `status` or `usage`,** and no `last ok` at the model scope.
- **No backfill.** A `usage.json` written before this change has no value, and its accounts read
  `never` until their next success.
- **No change to the counters' semantics or to any other `usage.json` field.**

## Acceptance criteria

- AC-1: A served request that succeeds sets the account's `lastSuccessAt`; a successful Refresh
  does not change it.
- AC-2: `lastSuccessAt` is written to `usage.json` with the account's counters, and a relay
  started on that file reports it in `GET /admin/accounts` before the account serves again.
- AC-3: A record whose credential is gone reports its stored `lastSuccessAt` rather than null.
- AC-4: A `usage.json` without the field loads, reports `lastSuccessAt: null`, and is rewritten
  with the field after the next success.
- AC-5: Rich `accounts` prints `last ok` as the first row of each account's block, as
  `<duration> ago` in the format `status` uses for uptime (`2m14s`, `3h5m`, `6d3h`), or `never`
  when `lastSuccessAt` is null. The row shares the block's label column.
- AC-6: Plain `accounts` appends `last_ok=<duration>` or `last_ok=never` to each account line,
  after `failures=` and `plan=`.
- AC-7: A `lastSuccessAt` later than the CLI's clock prints `0s`. An unparseable one prints
  `never`.
- AC-8: The row appears with and without `--verbose`.

## Verification

```
cargo test --locked last_ok
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
pengepul accounts | grep last_ok   # against the running relay, before and after a restart
```
