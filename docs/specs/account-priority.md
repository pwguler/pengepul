# account-priority

## Goal
Each Account in a Pool carries a Priority, and Rotation hands every new conversation to the most preferred Account that can serve, while a conversation already holding a cached prefix stays on its Account (ADR-0017).

## Non-goals
- No priority between Pools or Providers: one model id still resolves to exactly one Provider.
- Quota utilization never moves a request to a lower tier; only an Account that cannot serve does.
- No change to Cooldown, Retry hint, Reauth, Disabled, or Pool-of-one semantics (ADR-0020, ADR-0027).
- No change to how cache affinity is keyed (session header, `prompt_cache_key`, or prefix hash).
- No weights or traffic splitting; Priority is an order, not a share.

## Acceptance criteria
- AC-1: The config accepts an integer Priority per Account, keyed by Provider and Account key. A smaller number is preferred. An Account with no configured number gets the same default as every other unconfigured Account. An unknown Account key in that config is a startup warning, not an error.
- AC-2: A new conversation (no affinity yet) goes to the serving Account with the smallest Priority. Within equal Priority it goes to the Account whose weekly quota window (`7d`) resets soonest, as read from the vendor (ADR-0030). Accounts with no known weekly reset sort last within their tier; remaining ties break by Account key so the choice is deterministic.
- AC-3: An Account can serve unless it is on Cooldown, Disabled, or unavailable. When no Account in the best tier can serve, selection takes the next tier by the same rules. Near-exhausted quota alone never skips an Account.
- AC-4: A conversation with affinity stays on its Account while that Account can serve, even when an Account with a smaller Priority is available or recovers.
- AC-5: When the Account holding a conversation cannot serve, or Failover leaves it, the request goes to the best serving Account by AC-2 and AC-3, and affinity moves there (ADR-0017).
- AC-6: With no Priority configured anywhere, new conversations follow the AC-2 order (soonest weekly reset first); the round-robin cursor no longer decides them.
- AC-7: `pengepul accounts` shows each Account's effective Priority and whether it came from config or the default.
- AC-8: `CONTEXT.md` defines **Priority** and its Rotation entry states the new selection order; rubric judges that the glossary, README, and code agree.

## Verification
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `cargo fmt --check`
- rubric: AC-8
- Demo: a test fixture Pool of three Accounts with Priorities 1, 1, 2 and different weekly resets, showing new conversations land on the soonest-reset Priority-1 Account, a sticky conversation stays put after a Priority-1 Account recovers, and traffic reaches Priority 2 only while both Priority-1 Accounts are on Cooldown.

## Decisions so far
- [Where priority lives]: per Account inside one Pool, not between Pools.
- [Recovery vs cache]: cache wins; a sticky conversation stays on its lower-priority Account, new conversations go to the higher tier.
- [What drops a tier]: only Cooldown, Disabled, or unavailable; quota is not considered.
- [How priority is set]: an integer in config per Account, smaller is preferred, default equal for all.
- [Tie-breaker]: soonest weekly reset; no quota data sorts last in the tier; no round-robin within a tier.
