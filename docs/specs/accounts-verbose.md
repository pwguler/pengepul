# accounts-verbose

## Goal

`pengepul accounts` prints pools and accounts only; `pengepul accounts --verbose` (`-v`) adds
the per-model breakdown it printed before.

On the live relay the rich view ran to 349 lines, about 270 of them the four-to-five-row token
block repeated under every model an account had ever served. The account and pool blocks
already carry the same counters summed; the per-model split is a question asked rarely, so it
moves behind a flag.

## Non-goals

- **`--verbose` adds only the model scope.** Every line it prints outside the model headlines
  and model blocks is a line the default view prints, in the same order.
- **No change to the model headline or the model block,** which print as the model scope
  printed before this change.
- **No change to `status`, `usage`, `GET /admin/accounts` or `usage.json` from this spec.** The
  payload still carries `models`.
- **No hint in the output that models are hidden.** `pengepul help accounts` documents the flag;
  nothing else does.
- **No short flag other than `-v`, and no other detail hung on it.**

## Acceptance criteria

- AC-1: `pengepul accounts` in rich style prints no model headline and no model token block,
  for a payload whose accounts carry `models`.
- AC-2: `pengepul accounts` in plain style prints no model line and no indented model token
  line for the same payload.
- AC-3: `pengepul accounts --verbose` and `pengepul accounts -v` print every model headline and
  model block in both styles, and the model tests that predate this spec pass with the flag
  added to their argv.
- AC-4: The default output is the verbose output with the model lines removed: every account
  row, account block, pool header and pool footer prints the same with and without the flag.
- AC-5: `pengepul help accounts` lists `-v, --verbose` with a description naming the per-model
  breakdown.
- AC-6: `--verbose` combines with `--reload`.

## Verification

```
cargo test --locked --test cli accounts
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
pengepul accounts | wc -l; pengepul accounts -v | wc -l   # against the running relay
```
