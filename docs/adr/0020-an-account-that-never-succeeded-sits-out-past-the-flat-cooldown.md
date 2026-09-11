# 20. An account that never succeeded sits out past the flat cooldown

Status: Accepted (amends the Reauth and Cooldown entries in CONTEXT.md)

## Context

Failures are normally transient — a 429, a 503, a dropped connection — so an
account's cooldown is bounded: 1s, 2s, 4s, … capped at five minutes, or a flat ten
minutes for a billing rejection that will not clear mid-session. Both ceilings
assume the account is good and the error is not. For an account with no success to
its name that assumption is wrong in the same way every time: it will not clear,
so the ceiling is not a bounded price for retrying, it is a permanent retry rate.

Measured with `pengepul accounts` on the live `commandcode` pool:

| account | requests | successes | failures | cache read |
|---|---|---|---|---|
| key-4f84698d | 1541 | 1523 | 18 | 234.0M |
| key-90445c90 | 1220 | 1179 | 41 | 196.2M |
| key-d792179a | 91 | **0** | 91 | **0** |

The third key had never once succeeded, in eight days of traffic, across 91
requests — the same key ADR-0017's Context records at 58 requests, measured before
that change landed. Its failures are billing-scoped — exhausted credits — which is
why it was being drawn on a flat ten-minute cycle rather than the five-minute
transient one.
Six draws an hour, forever, each spending an upstream round trip for a reply that
cannot change.

Two details made this worse than a slow leak. `AccountManager` had **two** billing
paths: `record_failure(email, "billing", ..)`, which nothing on the wire reaches any
more (production 400/402 goes the refusal route and applies the cooldown through
`record_billing_cooldown`), and `record_billing_cooldown(email, ..)` that the failover
loop calls when a 400 or 402 body reports exhausted credits. The second assigned
`cooldown_until = now + 600s` unconditionally — no multiplier, no ceiling, and no "a
cooldown only ever grows" guard, so it could also collapse a 24-hour reauth cooldown
back to ten minutes, and it stamped its own reason over that cooldown's. It is the path
that fired for this key, so a rule applied only to `record_failure` would have fixed
nothing.

Under ADR-0017's original affinity key the cost was larger than the round trip. The
failure re-pinned the single affinity entry every conversation on that model shared,
so one dead key could move every conversation off the account holding its prefix.

CONTEXT.md documented the retry behaviour as deliberate: a rejected static key
"only cools down and comes back to fail again until the operator replaces it". That
sentence assumed the retry rate was the right price. It is not, for an account with
nothing to lose by waiting.

## Decision

The cooldown **ceiling** moves, and only for an account with no success to its name.
`AccountState::failure_cooldown` is the one place the pair is decided, and both
billing paths plus every other failure kind go through it:

```rust
fn failure_cooldown(&self, kind: &str) -> (f64, f64) {
    let billing = kind == "billing";
    if self.total_successes == 0 {
        let base = if billing { BILLING_COOLDOWN_SECONDS } else { FAILURE_BACKOFF.0 };
        (base, NEVER_SUCCEEDED_COOLDOWN_SECONDS) // 10min or 1s  … 1h
    } else if billing {
        (BILLING_COOLDOWN_SECONDS, BILLING_COOLDOWN_SECONDS) // unchanged: flat 10min
    } else {
        FAILURE_BACKOFF // unchanged: 1s … 5min
    }
}
```

`record_billing_cooldown` additionally takes the multiplier its sibling always had
and the `cooldown > state.cooldown_until` guard, so a repeat rejection lengthens
rather than re-assigning the same ten minutes, and a billing rejection can no
longer shorten a longer cooldown. For an account that has ever succeeded the
multiplier is inert — `600 * multiplier` capped at `600` is `600`, the value it
always assigned — but the *guard* is not: it also changes what happens when a
billing rejection arrives behind a longer cooldown, which is the bug fixed below.

For a never-succeeded account the base is unchanged, so the first rejection still
sits out ten minutes (billing) or one second (anything else), and only the ceiling
moves:

| consecutive failures | billing, never succeeded | transient, never succeeded | any kind, has succeeded |
|---|---|---|---|
| 1 | 10m | 1s | 10m / 1s |
| 2 | 20m | 2s | 10m / 2s |
| 4 | 1h | 8s | 10m / 8s |
| 13 | 1h | 1h | 10m / 5m |

The transient column needs 13 consecutive failures to reach the hour (`2^12` is
the first value past it), against 4 for billing. That is the intended asymmetry:
a credential whose balance is gone will not clear, while one that is merely erroring
should be retried eagerly until it has failed long enough to look dead.

Rejected: parking a never-succeeded account for a long window after a *fixed* small
number of failures. An upstream outage fails every account at once, so "three
failures and you are out for an hour" takes the whole pool down during a transient
that would have cleared in seconds. The ceiling has to be earned by a streak, not
granted by a counter; the failure counts above are reached only by failing
repeatedly while nothing else works.

Rejected: keying the rule on the failure kind or on the account being a static key.
It is more precise about this table's third row and it was the first design, but it
needs a second rule for the general case, and it cannot know that a 401 is not an
upstream hiccup. One ceiling applied by outcome history covers every kind without
asserting anything about *why* an account is failing.

## Consequences

- **A never-succeeded account stops being a recurring expense.** Steady state, a
  depleted key is retried hourly instead of every ten minutes — 24 probes a day
  instead of 144 — so a key that gets topped up upstream is picked back up within the
  hour rather than within ten minutes, without paying for the other 120 attempts.
- **The ceiling survives a restart; the streak does not.** `total_successes` is
  restored from persisted usage, so an account with no success to its name is still
  recognised as such after `pengepul serve` restarts. `failure_count` is not
  persisted, so the streak restarts at zero and the account is drawn four times
  (billing) before it climbs back to the hour ceiling. That ramp is bounded and
  accepted on its merits, not on a cost that does not exist: the write already
  happens on every outcome — `record_failure` and `record_billing_cooldown` both end
  in `persist_usage`, which rewrites the whole file — so persisting the streak would
  add a field, not a write. It is left out because four extra probes per restart is
  cheap, and because a restart is a clean slate for an account the operator may
  have just fixed.
- **The rule reads persisted history, so losing `usage.json` demotes every account.**
  `total_successes` comes from `~/.pengepul/<provider>/usage.json` and from live
  successes; nothing else. If that file is lost — an operator tidying up, a fresh host,
  a bad restore — every account sits at `total_successes == 0`, so a proven account
  becomes indistinguishable from a dead one and its ceiling moves from ten minutes to
  an hour. Measured: the same account with `usage.json` intact caps at `600s` after
  four billing failures; with the file removed, the same four failures give `3600s`.
  The rule is a policy decision resting on a reporting file, and the honest statement
  of its reach is that a relay whose history is gone will be slow to retry proven keys
  for as long as the histories stay gone.
- **What is protected is the load path, and only that.** `load_usage` maps an absent,
  truncated, malformed or non-object file to an empty map — a permissive contract the
  usage-persistence spec fixes for counters (its AC-5), and one this change does not
  overturn. What it does overturn is the write-back that followed: `AccountManager::load`
  now refuses to rewrite a file it could not read, and warns, so a startup that serves
  no traffic cannot destroy recoverable history and lock in the demotion. Two limits
  are worth stating plainly. A *served request* still rewrites that file, so recovery
  depends on the relay staying idle until the file is restored. And an entry dropped
  for an account whose token file was unreadable is still dropped from the next write,
  so repairing a token file after a bad read can leave that one account with no history.
  Both are pinned by tests (`an_unreadable_usage_file_is_not_overwritten_with_zeros_at_load`,
  `a_missing_usage_file_is_written_normally_not_treated_as_unreadable`). A rule that
  survives any of this would need a marker written once on first success into the token
  file itself, which is a change to what pengepul treats as durable and deserves its
  own decision rather than a rider on this one.
- **An account is delayed, never excluded — but a pool with no success to its name
  can be dark for an hour, and that is a real slowdown.** `account_for` still selects
  any account whose cooldown has expired, so a pool of one recovers by being selected
  again, and nothing here is a permanent exclusion; ADR-0017's rule that affinity never
  outranks availability is untouched. The honest limit is narrower than "availability
  cannot be lost": in a pool where *every* account has failed its way to the hour
  ceiling, the relay serves nothing until one expires. Nothing clears that hour except
  time, a successful request, a completed Refresh, a changed token file, or a restart —
  `reload` resets a cooldown only for an account whose credential changed, so topping
  up credits upstream without re-login does not bring the account back early.
  (A restart does not merely shorten the hour: `cooldown_until` is not persisted, so it
  clears the cooldown outright — pinned by `usage_counters_survive_a_manager_rebuild`,
  which asserts the account is `available` after a rebuild. That is the operator's
  fastest remedy, and it is why the ceiling is survivable at all.)
  The cost this replaces is worth naming rather than waving away: the previous server
  re-probed a never-succeeded pool every 600 s (billing) or 300 s (transient), so a
  condition fixed upstream was picked up within ten minutes; it can now take an hour.
  The pool was already refusing requests in both cases, so nothing that used to work
  stops working — but recovery is up to 6× slower for billing and 12× for transient
  failures, which is the price of not hammering a credential with nothing to lose.
- **A fresh account's first errors are still retried fast.** The change moves the
  ceiling, not the base, so the 1s/2s/4s opening that
  `failure_cooldown_doubles_from_one_second` pins is unchanged.
- **One success undoes all of it.** `record_success` resets `failure_count`, so a
  credential that was merely unlucky returns to the ordinary regime on its next
  successful request, and to full service immediately.
- **`record_billing_cooldown` no longer shortens a longer cooldown, and no longer
  mislabels one.** A billing rejection arriving after a 24-hour reauth cooldown now
  leaves that cooldown in place, and leaves the reason the operator reads —
  `refresh token …; re-run login` — intact rather than overwriting it with exhausted
  credits. Both are bug fixes, not design choices: the first is the invariant
  `record_failure` already documented and its sibling was missing, and the second is
  that same rule applied to the two fields `record_failure` keeps inside its guard,
  pinned by `a_billing_rejection_does_not_shorten_a_longer_cooldown`.
- **The operator still has to fix the credential.** This bounds the waste; it does
  not top up a balance or replace a key. `pengepul accounts` remains the place to
  see a zero-success row, and `systemctl --user restart pengepul` is the quickest
  way to clear an hour that a fixed credential has not yet earned back.
