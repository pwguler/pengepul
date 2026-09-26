# 26. A removed credential keeps its usage record

Status: Accepted

## Context

An upstream credential is deleted by hand — there is no verb for it — and `persist_usage`
wrote only the accounts whose files loaded. The next restart therefore dropped the deleted
key's counters out of `usage.json`, and with them the account out of the admin payload.
`status` and `usage` read that payload, so a deleted key silently removed the traffic it had
carried from every total, and nothing anywhere said so. On a live pool this cost one Anthropic
account 8,080 requests and 1.65B prompt tokens, and it surfaced only when the operator asked
why the relay's all-time total had gone backwards.

The loss is not recoverable after the fact. The relay keeps its counters in that one file, and
the per-request debug line that permits a partial rebuild was added late and is off by
default.

## Decision

A record whose credential is gone stays in `usage.json`, stays in the admin payload's
`accounts` array, and is listed and counted like any other account — one that cannot serve:
`available: false`, `cooldownUntil: 0`, no failure, no timestamps. Rotation, the failover
attempt budget and the startup account count read the serving map instead, so such a record is
never handed a request.

There is no `retired` flag, no second array, no new state and no new word. A reader that sums
accounts sums these, and a reader that lists accounts lists them, with no second rule.

## Considered options

- **Drop the record, as before.** Rejected: the traffic happened, the archive is the only place
  it exists, and the operator asked for it to be kept.
- **A `retired: true` flag, or a separate array the views mark.** Rejected by the operator:
  *"i dont want retired thing, just show as usual"*, then *"keep show the deleted key as
  well"*. A second shape also means a second rule in every reader, including the two that only
  sum.
- **Keep the record in the file but out of the payload**, so only the disk remembers. Rejected:
  the file is not a surface, and the ask was to see it in `status` and `usage`.

## Consequences

- `account_count` is the length of the array it counts, so the header and the per-pool lines
  cannot disagree with the rows a client prints.
- The startup log line still counts loaded accounts rather than records: `generic=2` beside a
  payload listing three is correct, and it is the line that says what the relay can serve.
- A carried record is not repaired at load. `reconcile_loaded_counters` closes attempt/outcome
  gaps for an account whose health the counters decide; a record with no credential is reported
  as the file holds it.
- Retention reaches the record on the same write and the same cutoff as any account's daily
  buckets, and deleting `usage.json` remains the only reset.
- `lastSuccessAt` is the exception to "no timestamps": last-ok persists it with the counters,
  so a record without a credential reports when it last served.
