# round-robin-rotation

## Goal
A Pool can opt into round-robin Rotation, which reads no Priority and hands each new conversation to the next serving Account in Account-key order, while a conversation already holding a cached prefix stays on its Account (ADR-0017, ADR-0033).

## Non-goals
- No change to Rotation by Priority, the default: its order, its tie-breakers and how the views print it stay as they are (ADR-0031).
- No change to Cooldown, Retry hint, Reauth, Disabled, Pool-of-one (ADR-0020, ADR-0027) or Failover semantics.
- No change to how cache affinity is keyed (session header, `prompt_cache_key`, or prefix hash), nor to its bound.
- No round-robin inside a Priority tier, no weights, and no spreading per request: round-robin places conversations.
- No persisted cursor.
- `/admin/reload` re-reads `rotation:`; `priority:` keeps needing a restart (ADR-0031).

## Acceptance criteria
- AC-1: The config accepts a top-level `rotation:` map from Provider id to `priority` or `round-robin`. A Provider the map leaves out rotates by `priority`. Any other value refuses the config at load, and the error names `rotation`. A Provider id the relay does not serve is a startup warning, never a refusal. A config without `rotation:` loads as before, and a config the relay rewrites gains no `rotation:` key and keeps one it had.
- AC-2: In a round-robin Pool, new conversations (no affinity) go to the serving Accounts in Account-key order, starting at the first key and wrapping after the last, so k serving Accounts take N×k new conversations N each. Priority and the weekly reset do not change that order.
- AC-3: A conversation with affinity stays on its Account while that Account can serve, and its turns move no cursor.
- AC-4: The cursor passes over an Account on Cooldown, Disabled, or without a credential, and the next serving Account in key order takes the conversation.
- AC-5: When the Account a conversation holds cannot serve, or Failover leaves it, the request goes to the next serving Account by the cursor, that placement moves the cursor, and the conversation's affinity moves there.
- AC-6: The cursor is one per Pool and lives in memory: a rebuilt Pool starts again at the first key, and concurrent new conversations each take a turn of their own.
- AC-7: A `priority` Pool beside a round-robin Pool hands out new conversations exactly as before.
- AC-8: `POST /admin/reload` applies the `rotation:` that the config file the relay loaded holds now. A file that no longer parses refuses the reload with the parse error and changes nothing.
- AC-9: `/admin/accounts` names each Pool's rotation. `pengepul accounts` names a round-robin Pool in its header, lists its Accounts in key order, and prints no Priority for them, in both styles; a `priority` Pool prints as before.
- AC-10: `CONTEXT.md` defines **Round-robin**, and its Rotation, Priority and Pool entries agree with it; README documents `rotation:` with an example; rubric judges that the glossary, README and code agree.

## Verification
- `cargo fmt --check`
- `cargo clippy --locked --all-targets --all-features -- -D warnings`
- `cargo test --locked`
- rubric: AC-10
- Demo: a second relay from the branch on `127.0.0.1:18418`, from a temp copy of the live config plus `rotation: { anthropic: round-robin, commandcode: round-robin }` and a temp copy of the auth dir. Fresh conversations to commandcode (distinct `x-session-id`, a tiny prompt, the cheapest model) land on its three keys in key order, a repeated session key stays on its key, and `pengepul accounts` and `/admin/accounts` name both Pools round-robin. The live anthropic Pool serves one Account, so it shows the setting and the stickiness, not a spread.

## Decisions so far
- [Scope]: per Pool, keyed by Provider id under a top-level `rotation:`; `priority` is the default.
- [What round-robin reads]: no Priority and no weekly reset; `priority:` stays valid and unread.
- [Cache affinity]: unchanged; only a placement (a new conversation, or one leaving an Account that cannot serve) takes a turn.
- [Cursor]: one per Pool, in memory, deterministic by Account key, restarting at the first key on relay start; it skips Accounts that cannot serve.
- [Reload]: `/admin/reload` re-reads `rotation:`.
