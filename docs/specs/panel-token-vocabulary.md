# panel-token-vocabulary

## Goal

Every token figure the CLI prints carries one name with one meaning: a panel block of
`input` / `cached` / `uncached` / `output` at every scope, `reasoning` after `output`, a share
on `cached` that is the read share of the prompt, and no cache write anywhere in the CLI
(ADR-0024).

## Non-goals

- **No change to what is recorded.** `input_tokens` stays the prompt token no cache read or
  wrote, the 1h share stays recorded, and `usage.json` keeps its format and its field names.
  ADR-0023's counter semantics are untouched.
- **No rename on the wire.** `totalInputTokens`, `totalCacheReadInputTokens`,
  `totalCacheCreationInputTokens` and `totalCacheCreation1hInputTokens` keep their names and
  values in `GET /admin/accounts` and in `usage.json`
- **No cost or pricing in the CLI.** The relay prices nothing; the 1h share leaves the panels
  rather than becoming a priced figure.
- **No change to the `usage` trend's rows.** `tokens` / `peak` / `window` / `all time` are
  carried load, not a prompt, and none of them is a counter this change renames.
- **No change to the model picker or `launch`.**
- **`consistent-panels`' cooldown wording is not reopened.** Plain `accounts` keeps printing
  `unavailable` for an account with no future cooldown. This change breaks plain's bytes for
  the token block only; the cooldown spelling is a separate decision with its own ADR.
- **No per-model windowed history.** The blocks read the same all-time counters the panels
  read today.

## Acceptance criteria

- AC-1: Every scope that reports a subject prints the block in order — `input`, `cached`,
  `uncached`, `output` — with `reasoning` after `output` when non-zero: the relay block in
  `status`, and the pool, account and model blocks in `accounts` (ADR-0024); the model block under
  `--verbose` since accounts-verbose.
  `tests/cli.rs::accounts_renders_the_token_block_at_every_scope` asserts the exact label
  sequence at the account, model and footer scopes, and
  `tests/cli.rs::status_renders_panels_on_a_tty` asserts the relay's own block.
- AC-2: `cached` carries `read / input`, not the two cache directions summed.
  `the_cache_share_is_the_read_share_of_the_prompt` pins `cache_with_share` for a fixture
  whose cache write is non-zero, and asserts the value the pre-change code printed is *not*
  produced: 155.0M read of a 183.1M prompt is `cached 155.0M (85%)`, where the pre-change code
  printed `161.0M (88%)`.
- AC-3: `uncached` prints the never-cached input plus the cache write, with no parenthetical
  and no trailing breakdown. For the anthropic fixture, input 22.1M + write 6.0M renders
  `uncached 28.1M`, and a parenthetical there would fail the exact-equality assertions.
- AC-4: `input = cached + uncached` holds for every printed subject. A test derives the three
  figures from the fixture's counters and asserts the rendered strings satisfy it, so a
  subject whose three rows do not agree fails rather than prints.
- AC-5: No command prints a cache write or its 1h share. The strings `1h write`, `cache
  write` and `written to cache` occur in no rendered output of `status`, `accounts` or
  `usage`; `tests/cli.rs::no_command_prints_a_cache_write` greps the golden output of all
  three, plain and rich.
- AC-6: No panel prints a `total` or a `pool` fact row. `status`'s relay block and
  `accounts`' pool footer end at `reasoning`; `carried_tokens` remains the pool-row and
  model-headline figure in both styles.
- AC-7: Plain output carries the same four words, one line per subject, with `output` spelled
  as in the panel: `input … cached … (…) uncached … output …`, and `reasoning` when non-zero.
  `tests/cli.rs::status_rolls_up_pool_health_and_token_totals_per_provider` asserts the
  relay's line, and `::accounts_detail_prints_usage_and_cooldown_per_account` and
  `::accounts_lists_models_in_plain_output` assert the account's and the model's.
- AC-8: Plain output is no longer byte-identical to the previous release, and the
  `tests/cli.rs` assertions that pinned the old bytes are rewritten deliberately rather than
  deleted: each rewritten assertion names the label it now expects. No test function is
  removed. `tests/accounts.rs` asserts the recorded counters, which this change does not
  touch, so it is unchanged and still green. The version in `Cargo.toml` is bumped.
- AC-9: `CONTEXT.md` defines **Input**, **Cached** and **Uncached**, states that the recorded
  `input` counter is narrower than the printed `Uncached` by the cache write, lists `in` and
  `cache` as avoided spellings, and no longer describes the plain contract as byte-stable.
- AC-10: `ARCHITECTURE.md`'s panel rules match the output, at all three places that state
  them: the restatement rule ("A row reports a fact the panel does not already carry") admits
  a labelled total above its own breakdown; "One word, one scope" no longer claims `status`
  prints an all-time figure beside `usage`'s; and the `Style` bullet no longer claims piped
  output is byte-stable for scripts, which ADR-0024 gave up.
- AC-11: A fact row may open an indented breakdown beneath it, one level per scope: the token
  block sits at two spaces under an account row and at four under a model headline, with the
  pool footer's at the panel's own margin. Inside a block the value column is one offset from
  that block's indent, not shared with the scopes around it — the indent is what says whose
  rows these are. `tests/cli.rs::accounts_renders_the_token_block_at_every_scope` asserts the
  block's order at each scope, and every panel row still measures 64 columns
  (`usage_view::tests::rich_renderer_panels_are_exactly_the_fixed_width`).

## Verification

```
cargo test --test cli
cargo test --test accounts
cargo test --lib usage_view
cargo test --locked --all-targets --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo fmt --check
```
