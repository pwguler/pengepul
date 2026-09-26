# accounts-verbose

## Goal

`pengepul accounts` prints pools and accounts only; `pengepul accounts --verbose` (`-v`) adds
the per-model breakdown it printed before.

On the live relay the rich view ran to 349 lines, about 270 of them the four-to-five-row token
block repeated under every model an account had ever served. The account and pool blocks
already carry the same counters summed; the per-model split is a question asked rarely, so it
moves behind a flag.

## Non-goals

- **No change to what `--verbose` prints.** Its bytes are the pre-change `accounts` output,
  in both styles.
- **No change to the account row, the account block, the pool header or the pool footer.**
- **No change to `status`, `usage`, `GET /admin/accounts` or `usage.json`.** The payload still
  carries `models`.
- **No hint in the output that models are hidden.** `pengepul help accounts` documents the flag;
  nothing else does.
- **No short flag other than `-v`, and no other detail hung on it.**

## Acceptance criteria

- AC-1: `pengepul accounts` in rich style prints no model headline and no model token block,
  for a payload whose accounts carry `models`.
- AC-2: `pengepul accounts` in plain style prints no model line and no indented model token
  line for the same payload.
- AC-3: `pengepul accounts --verbose` and `pengepul accounts -v` print every model headline and
  model block in both styles, and the model tests that predate this spec pass unchanged except
  for the flag added to their argv.
- AC-4: Without the flag, every account row, account block, pool header and pool footer is
  printed exactly as with it: the default output is the verbose output with the model lines
  removed.
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
