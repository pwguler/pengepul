# prose-accuracy-and-test-sensitivity

## Goal

Make three documented claims match the code they describe, and make three tests
sensitive to the thing they claim to check.

## Non-goals

- **No production behaviour change.** No policy, cooldown value, key derivation,
  wire output or routing change. The only production-code edits are comments.
- **`system` and `tools` stay unbounded in `conversation_key`.** Bounding them is
  the thing this work decided against, and AC-3 records why.
- **Identifiers are not renamed.** Test function names keep their spelling even
  where they use an avoided term; only prose is in scope.
- **The `"account selected"` debug message stays byte-identical.** It describes an
  event, not a name for Rotation, and an operator may grep it.
- **The never-succeeded ceiling keeps reading `total_successes`.** No durable
  marker is added to the token file.
- **`tests/cli.rs`'s "service status stays an error in both styles" assertion is
  left as-is.** Its contract is that it is an `Err` and not a panel, and it pins
  no message by design.

## Acceptance criteria

- AC-1: ADR-0020 states the blast radius of a lost `usage.json` as bounded by each
  account's next success, and no longer says the relay is slow to retry "for as
  long as the histories stay gone".
- AC-2: A test proves that bound: after loading with no `usage.json`, a first
  failure caps at the never-succeeded ceiling, and one recorded success returns the
  same account to the ordinary billing ceiling.
- AC-3: The `conversation_key` comment states that leaving `system` and `tools`
  unbounded is deliberate, naming the collision that bounding `system` would
  reintroduce, rather than presenting it as an unexamined cost.
- AC-4: ADR-0017 records the same as a decision, so the unbounded hash does not
  read as an oversight next to the bounded message window.
- AC-5: No prose in `src/` or `docs/` uses "lockout"; the term appears only in test
  identifiers, which are explicitly exempt.
- AC-6: ADR-0021's call-site count reproduces. The claim "99 call sites" was false,
  produced by a `grep` for `run(&[` that missed the `run(argv, ...)` form; the count
  at the revision the defect existed in is 118 (66 `run(` plus 52 `run_style(`,
  definitions excluded), and the file says so together with the 120 this change
  leaves behind.
- AC-7: The account-selection debug-line test fails when any one of
  `conversation`, `provider` or `account` is replaced by a constant, not only when
  `affinity` is.
- AC-8: `login_without_key_for_a_configured_provider_fails` asserts the refusal
  reason rather than only that the call failed.

## Verification

```
cargo test --locked
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo clippy --locked --all-targets --all-features -- -D warnings
grep -rn "lockout" src/ docs/ README.md CONTEXT.md ARCHITECTURE.md   # prose only, AC-5
```

AC-7 is verified by mutation: substitute a constant for each of `conversation`,
`provider` and `account` in the `tracing::debug!` call and confirm the test fails
for each. AC-2 is verified by reverting the assertion's subject and confirming the
test fails. AC-1, AC-3, AC-4, AC-5, AC-6 and AC-8 are statements about text and are
verified by reading the file and by the commands above.
