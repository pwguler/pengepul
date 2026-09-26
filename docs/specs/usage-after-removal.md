# usage-after-removal

## Goal

Removing an upstream credential from the auth directory stops the account serving and
changes nothing else. Its usage record stays in `usage.json`, stays in the payload as an
account entry, and every view that counts or lists accounts goes on counting and listing
it: `status`'s pool lines and relay total, `usage`'s trend and `all time`, and `accounts`'
rows and per-model detail.

The user's words: *"if the key deleted, all the usage meter deleted/ignored, i want you
keep it in usage and status"*, then, on the shape: *"i dont want retired thing, just show
as usual"* and *"keep show the deleted key as well"*.

Before this change the record died with the credential. A credential is deleted by hand —
there is no verb — and the next `persist_usage` wrote only the accounts whose files
loaded, which the code said outright:

```rust
// Only loaded accounts are written: entries in the file for
// unknown emails are dropped rather than carried forever.
```

The payload's `accounts` array is built from loaded accounts alone, so the account then
disappears from `status`, from `usage` and from `accounts` at once.

## Non-goals

- **No new state, marker or word.** No `retired`, no `gone`, no sixth account state, no
  second count. The record is listed and counted like any other, and reads as an account
  that cannot serve: `available: false`, no Cooldown, no error, no timestamps.
- **No verb for removal.** `rm ~/.pengepul/<provider>/key-<hash>.json` stays the way to
  do it; `login`'s own comment already calls that "the normal state".
- **Serving is unchanged.** Rotation, Failover and the per-provider attempt budget walk
  the accounts that hold a credential. A record without one is never handed a request and
  never earns a Cooldown.
- **`reload` still never removes an account.** A running relay keeps serving a credential
  whose file was deleted until it restarts; classification happens at load, over what the
  auth directory holds.
- **No new retention rule.** The record's daily buckets are trimmed to the same 90-day
  window, by the same write and the same cutoff, as any account's. Deleting `usage.json`
  remains the only reset.
- **A removed `providers:` entry is out of scope.** That drops the pool from the payload,
  records and all, and is a separate ask.
- **A carried record is not repaired.** `reconcile_loaded_counters` closes attempt/outcome
  gaps for an account that can serve, because a gap there is the account's health. A
  record with no credential has no health to repair and no request to settle: it is
  reported as the file holds it, never rewritten to balance.
- **No new payload shape.** The entry carries the field names every account entry already
  carries, so no reader needs a second shape and no view needs a second rule.

## Design

- `persist_usage` writes the accounts that loaded from memory, then carries every other
  entry in the file through unchanged. The file is the record of what the relay carried,
  not only a snapshot of what it can serve.
- `AccountManager::snapshots` merges both sets into one list, in account-key order. A
  record without a credential is emitted with the liveness defaults of an account that
  cannot serve: `available: false`, `cooldownUntil: 0`, `failureCount: 0`, null
  `lastError`/`lastFailureAt`/`lastRefreshAt`/`planType`, empty `expiresAt`, and the
  `lastSuccessAt` the file holds (last-ok made it a persisted record, not liveness).
- `GET /admin/accounts` counts that list rather than calling `account_count()` separately,
  so the header's `N accounts`, the per-pool lines and the rows a client prints cannot
  disagree about how many there are.
- `next_account`, `account_count` and the attempt budget keep reading the serving map,
  which holds credential-holding accounts only.

## Acceptance criteria

- AC-1: A credential file removed and the relay restarted keeps its entry in
  `usage.json` — cumulative counters and daily buckets — and every later write carries
  it.
  - Verify: load over a static credential, record traffic, delete the token file, load a
    fresh manager. The entry is in the file; then record traffic for a surviving account
    and re-read the file: the record is still there, counters unchanged.
- AC-2: The payload lists the record in `accounts` with its usage fields and the liveness
  defaults above, and `account_count` counts it — so `status`'s pool line and relay
  aggregate, and `usage`'s trend and `all time`, include its requests and tokens.
  - Verify: a CLI test over a payload holding one account with a credential and one
    without; assert both are counted and the count matches the rows.
- AC-3: `pengepul accounts` lists it in both styles with the existing vocabulary and no new
  field: plain prints the row, its token line and its per-model lines, rich prints it as an
  account that cannot serve, with its share of the pool.
- AC-4: Rotation never hands it out, and it is not a Cooldown: `next_account` returns only
  credential-holding accounts over a full cycle, including when every one of those is on
  Cooldown.
  - Verify: a manager over an auth dir holding one credential and one usage record; drain
    `next_account` (once per account plus one) and assert the record's email never comes
    back.
- AC-5: Re-adding the same credential restores the record instead of starting at zero. The
  label is derived from the key (`key-<hash of the key>`), so the same key returns to the
  same entry, and the account serves again with its counters.
- AC-6: A `usage.json` that does not parse is still left untouched and yields nothing,
  live or otherwise. The existing guard is what keeps a corrupt file recoverable, and
  nothing is invented from it.
- AC-7: The payload keeps its existing shape: `account_count` equals the length of
  `accounts`, every entry carries the same field names as before, and no other field is
  added or renamed.

## Verification

```sh
cargo fmt --check
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
```

Driven against a live relay, with a static-key provider that has served traffic:

```sh
cp ~/.pengepul/commandcode/key-90445c90.json /tmp/key.json
pengepul status | tail -3
pengepul accounts | grep -A2 key-90445c90
rm ~/.pengepul/commandcode/key-90445c90.json
systemctl --user restart pengepul
pengepul status | tail -3                 # the same totals
pengepul usage                            # the trend still has that key's days
pengepul accounts | grep -A2 key-90445c90  # listed, unavailable
jq 'keys' ~/.pengepul/commandcode/usage.json   # the entry is still there
pengepul login --provider commandcode --key "$(jq -r .access_token /tmp/key.json)"
pengepul accounts | grep key-90445c90     # available again, same totals
```
