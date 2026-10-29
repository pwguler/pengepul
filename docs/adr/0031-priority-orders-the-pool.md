# 31. Priority orders the Pool, and a cached conversation outranks it

Status: Accepted (amends the Rotation entry in CONTEXT.md and ADR-0017)

## Context

ADR-0017 made Rotation keep a conversation on the account that holds its cached prefix, and
left the choice of account for a *new* conversation to a modulo cursor over the rotation
order. That cursor is fair and says nothing. In a Pool where one subscription is better — a Max
plan beside a Pro one, a key with credits beside one about to run out — the cursor spends half
the new conversations on the account the operator wants used last, and nothing in the config
could say so.

The vendor now states each account's quota windows (ADR-0030), including the weekly one's reset
instant, which is the only per-account fact that says "this account's cycle is about to
refresh". The operator asked for a rank that uses it, and settled five points before any code
was written:

- Priority is per **Account**, inside one Pool. Nothing compares across Pools or Providers: one
  model id still resolves to one Provider, so there is no Pool to compare it with.
- Smaller is preferred, and every Account the config leaves out shares one default, so a
  partial config ranks relative to that default rather than to nothing.
- Within equal Priority, the Account whose weekly (`7d`) window resets soonest; one with no
  usable reading last; the Account key as the final tie.
- No round-robin inside a tier. Equal Priorities are not interleaved.
- Quota utilization is never a reason to leave an Account. Only a Cooldown, Disabled, or a
  missing credential drops the request to the next tier.
- Cache affinity wins over Priority: a conversation stays on its Account while that Account can
  serve, even after a more preferred Account recovers.

## Decision

`AccountManager` holds a Priority per Account key, read from a `priority:` map in `config.yaml`
keyed by Provider and then Account key. The effective Priority of an Account the map leaves out
is `0`; the map is fixed at startup, and a key the Pool does not hold is a warning that names it,
never a refusal to load (AC-1). Selection is:

```text
serving set  = accounts in the rotation order (a credential, not Disabled) off Cooldown
new work     = min_by(priority, weekly_reset_then_none_last, account_key) over the serving set
a conversation with affinity = its recorded account, while that account is in the serving set
next tier    = the same order without the accounts that cannot serve
```

The weekly reset comes from the stored `QuotaSnapshot`'s `week` window (`quota.json`, or the
header of the last response) and is read only when it is still in the future: a window whose
reset has passed prints `n/a` (ADR-0030) and says nothing about the next cycle, so it sorts
with the Accounts that have no reading at all. Utilization is not read by selection at all —
`usedPct`, a `rejected` status, even `LIMIT`, leave the order untouched.

The modulo cursor is **deleted** (`last_used_index`), not kept as a final tie: AC-6 says it no
longer decides, and the Account key is the deterministic last word.

## Considered options

- **A separate `accounts.rank` file.** Rejected: the config is already the operator's one place
  for policy, and a second file is a second thing to keep in sync with the auth directory.
- **Priority as a share (weights).** Rejected by the operator: the request was an order, and a
  share reintroduces the interleaving within a tier that the tie-break exists to remove.
- **Skip an Account near its quota cap.** Rejected: the vendor's own numbers move during a
  cycle, so a local threshold would take an Account out for a reading the vendor has not
  refused, and the relay would hide a 429 the client should see. The vendor's `rejected` status
  already earns a Cooldown through the failure path.
- **Round-robin among equals.** Rejected: it makes the chosen Account depend on history, so two
  identical Pools answer differently after a restart, and the demo below could not be written
  as an assertion.
- **Let a recovering higher-Priority Account pull its conversations back.** Rejected by the
  operator, and by ADR-0017's cost model: a pull-back re-bills the prefix the other Account just
  cached. New work goes to the preferred Account; running work stays.

## Consequences

- **The cursor was spreading load, and now nothing does.** With equal Priorities and no quota
  reading — every static-key Pool, and any Pool on a Provider whose usage route is unknown — new
  conversations all land on the lowest Account key until it cannot serve. This is the settled
  design, not an oversight: `a_tie_on_priority_and_reset_breaks_by_account_key` and
  `without_a_configured_priority_new_conversations_do_not_alternate` pin it. The mitigation is
  the operator's to apply — rank the accounts — or the weekly reset's: on a Provider that
  reports `7d`, the preferred Account's reset passes, its window reads `n/a`, it sorts last and
  the next tier takes new work.
- **Two tests that asserted the old fairness were rewritten, not weakened.**
  `messages_route_rotates_available_anthropic_accounts` became a configured-Priority test, and
  `a_different_conversation_still_takes_the_next_account` now asserts that two new
  conversations share the preferred Account. `one_chat_conversation_failing_over_does_not_re_bill_another`
  had to seed its two conversations on one Account (the new contract) and clear the Cooldown
  through `POST /admin/accounts/enable` to show the sibling's prefix survived a failover.
- **The selection log's `honored`/`rotation` vocabulary is unchanged**, and its test now parks
  one Account to reach the second with a new conversation instead of relying on the cursor.
- **An Account added by a later login picks its configured Priority up at the next start,** not
  at `--reload`: the map is read once, with the rest of the config, and the startup warning says
  the key is waiting.
- **One number, one home.** The payload carries `priority` and `prioritySource` (`config` or
  `default`) on every record, including one whose credential was removed (ADR-0026), so
  `pengepul accounts` — plain and rich — and the Windows tray read the same pair and cannot
  disagree about which number is configured (AC-7).
- **Nothing else moved.** Cooldown, Retry hint, Reauth, Disabled and Pool-of-one semantics are
  untouched (ADR-0020, ADR-0027), and so is the affinity key and its 1024-conversation bound
  (ADR-0017).

## Verification

The spec's demo fixture lives in
`the_demo_pool_lands_on_the_soonest_reset_tier_then_holds_its_sticky_conversation`: three
accounts of one Pool, Priorities 1, 1, 2, weekly resets at +5 days, +1 day and +3 days. New
conversations land on the `+1 day` account of tier 1; with it on Cooldown a conversation pins
to the `+5 day` account, and stays there after the preferred one is cleared; only while both
tier-1 accounts are cooling is the Priority-2 account served.

`cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` pass. The
glossary, the README and the code agree on **Priority** (AC-8).
