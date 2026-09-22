# single-account-pool-cooldown

## Goal

A Pool that holds one account keeps serving it through every failure, so the client is
answered by the vendor instead of by the relay. A Cooldown exists to hand the next request
to a sibling; with no sibling it can only withhold the last way the Provider has to serve.

The user's words: *"if only one account in a pool no need to cooldown"*.

The reasoning and the rejected alternatives are in ADR-0027.

## Non-goals

- **No change to a Pool of two or more.** Rotation, Failover, all three cooldown policies
  and ADR-0020's never-succeeded ceiling behave exactly as before.
- **No change to what a failure records.** `failure_count`, `total_failures`, the daily
  bucket, `lastError`, `lastFailureAt` and `last_failure_kind` are written as they are for
  any account.
- **No new state, flag or word.** Nothing marks an account as "lone"; the rule reads the
  rotation set.
- **No reader change.** `next_account_result`, `account_for` and `snapshots` keep reading
  `cooldown_until` unchanged. Cooldowns are not persisted and the rotation set only grows
  inside a process, so the writer is where the rule can be held.
- **No change to `pengepul accounts`, `status` or `usage`.** An account that failed and is
  still serving shows its climbing failure count in Rotation, and the payload's `lastError`
  names the reason; those views already print both for any account.

## Acceptance criteria

- AC-1: With one account loaded, `record_failure` writes the failure streak, the counters
  and `lastError`, and leaves `cooldownUntil` at `0.0`, for every failure kind.
- AC-2: With one account loaded, `record_billing_cooldown` and `record_refresh_exhausted`
  do the same — the rare kinds earn no park either, and the reauth message
  (*"re-run login"*) still reaches `lastError`.
- AC-3: `next_account_result` keeps returning that account: `next_account` yields it after
  any sequence of failures, so the relay never answers `no_account_for_provider` for a
  Provider that holds an account.
- AC-4: `snapshots` reports `available: true` and `cooldownUntil: 0.0` for it, and the
  counters (`failureCount`, `totalFailures`, `lastFailureAt`) agree with AC-1 and AC-2.
- AC-5: At the HTTP surface, two consecutive Messages requests through a relay whose one
  account the upstream rejects with 429 both reach the upstream and both answer the client
  with the upstream's own 429 and body.
- AC-6: Adding a second credential restores the previous behaviour: the cooldown is earned
  again, `available` goes to `false` and rotation falls through to the sibling.
- AC-7: A Pool of two keeps every rule it had. The cooldown-mechanics tests
  (`failure_cooldown_doubles_from_one_second`,
  `a_credential_that_never_succeeded_is_parked_past_the_transient_error_ceiling`,
  `a_depleted_key_that_never_succeeded_escalates_past_the_flat_billing_cooldown`,
  `a_billing_rejection_does_not_shorten_a_longer_cooldown`,
  `a_reauth_lockout_is_not_clobbered_by_the_paired_failure`,
  `a_lost_usage_file_demotes_until_the_accounts_next_success`,
  `counters_written_as_floats_or_strings_still_count`) run on two-account fixtures and pin
  the same numbers they pinned before.
- AC-8: A record whose credential is gone is still never handed a request, including when
  every usable account is on Cooldown (`rotation_never_hands_out_a_record_without_a_credential`).

## Verification

```sh
cargo test --test accounts
cargo test --test app a_lone_account_is_never_parked_out_of_rotation
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

AC-1 to AC-4 and AC-6 are `a_pool_of_one_never_earns_a_cooldown` in `tests/accounts.rs`;
AC-6's fall-through to the sibling is also pinned by
`a_pinned_account_on_cooldown_falls_through_to_rotation`. AC-5 is
`a_lone_account_is_never_parked_out_of_rotation` in `tests/app.rs`, which drives the route
over a fake upstream. AC-7 is the seven tests it names; AC-8 is
`rotation_never_hands_out_a_record_without_a_credential`.

Manual, against a live relay with one account per pool: run a request the vendor rejects
(e.g. a model the account cannot use), then confirm the admin payload reports the account
with `available` true and the reason in `lastError` — `pengepul accounts` prints it in
Rotation with a climbing failure count — and that the next request reaches the vendor
rather than answering `503 no_account_for_provider`.
