# 27. A Pool of one keeps no Cooldown

Status: Accepted (amends the Cooldown entry and the account invariants in CONTEXT.md)

## Context

A Cooldown is a Rotation signal. It says *hand the next request to the sibling*: the
account's own error is transient or scoped to the credential, another account is a better
bet for this request, and the rejected account sits out a bounded time instead of being
asked again immediately.

A Pool that holds one account has no sibling. Parking that account withholds the only way
the Provider has to serve: `next_account_result` finds no eligible account and returns
`None`, so `next_provider_account` answers with its own
`503 no_account_for_provider` — *"no available anthropic account; run login for anthropic;
last failure: rate_limit; retry after 600 seconds"* — without anything leaving the process.

Both halves of that message are worse than the error it replaces. The vendor's own 429 says
what happened and carries its own retry hint; a local 503 names a failure the client cannot
see and a wait it cannot shorten, and it makes the relay's availability look like a missing
credential. The operator's runbook for that message is `login`, which is the wrong advice
for an account that exists, is authorized, and is merely being throttled.

The three duration policies all end this way for a lone account — the ordinary failure
cooldown, the flat billing one, and the 24-hour reauth one — and ADR-0020's never-succeeded
ceiling ends it for an hour.

## Decision

**A Pool that holds exactly one account earns no Cooldown.** All three duration policies
skip the `cooldown_until` write when the rotation set holds one account:
`record_failure`, `record_billing_cooldown` and `record_refresh_exhausted`.

Everything else the failure records still happens. `failure_count` climbs, `total_failures`
and the daily bucket count the outcome, and `lastError` and `lastFailureAt` are written, so
`pengepul accounts` counts the failures and keeps the account in Rotation, and the payload
carries the reason in `lastError` beside them.
What does not happen is the park: the next request is handed the same account, reaches the
vendor, and the client is answered with the vendor's own error.

The rule keys off `order`, the rotation set — the accounts that hold a credential. A record
whose credential was deleted is not in it (usage-after-removal), so a Pool listed with two
rows but one credential is a Pool of one.

No reader changes with it. `next_account_result` and `account_for` keep honouring
`cooldown_until` as they always have, because a cooldown is never persisted: the only way a
lone account could hold one is if it were earned while a sibling existed, and `order` only
ever grows inside a process — a delete is picked up at the next load, which starts from an
empty manager. The invariant is therefore held by the writer, and the reader is left to do
one thing.

## Considered options

- **Earn the Cooldown, ignore it in selection.** Rejected: `snapshots` derives `available`
  and `cooldownUntil` from that field, so `accounts` would print the only account as
  `on cooldown <remaining>`, and the payload would carry a retry-after, for a request the
  relay serves anyway.
- **Wait out the shortest Cooldown instead of failing.** Rejected: it holds a request open
  for up to an hour to hide a vendor error the client should see, and it breaks the
  relay's one rule about time — the client owns its own retry policy.
- **Keep the Reauth cooldown even for a lone account, so a dead refresh token does not cost
  a doomed refresh per request.** Rejected by the operator: a dead credential is precisely
  when a human needs the message that says to re-run `login`, and the local 503 hides it.
  The cost is bounded by the client's own request rate, and the refresh is only attempted
  when the access token is due.
- **Never cool down at all.** Rejected: a Pool of two or more is exactly what the mechanism
  is for, and ADR-0020's measurement — a never-succeeded key drawing 91 requests and serving
  none — was taken on a Pool of three.

## Consequences

- A lone failing account now spends one upstream round trip per client request. That is what
  a client talking to the vendor directly would spend, and it is bounded by the client's rate
  rather than by a timer.
- `next_account_result` can no longer return `None` for a Provider that holds an account, so
  the `retry_after_seconds` half of `no_account_message` is only reachable for a Pool of two
  or more whose accounts are all cooling — where a sibling does exist and waiting is the
  operator's decision.
- Adding a second credential restores the old behaviour on the next request: `order` grows,
  the Pool has siblings, and every duration policy applies again.
- Panels need no new word. An account that failed and is still serving reads as an account in
  Rotation with a climbing failure count, and the payload's `lastError` names the reason.
