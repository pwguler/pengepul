# cli-seams

## Goal

`cli.rs` becomes command dispatch only. The three concerns that grew inside
it move behind their own seams, and the usage file joins the Credential
store — with every observable behavior of the CLI and the relay unchanged.

## Shape (settled in the architecture interview)

- **`render.rs`** — the panel language and nothing else: `Style`, palette
  and `paint`, `panel_row`, `top_rule`/`action_rule`/`action_panel`,
  `status_glyph`/`ActionGlyph`, `share_bar`, `pad`, `strip_ansi`,
  `format_count`/`format_exact`/`format_duration`, the width constants,
  and `Output`. It knows nothing of Pools, Accounts, or the admin payload.
- **`usage_view.rs`** — payload → lines for `status`/`accounts`:
  `PoolTotals`, `RelayTotals`, `account_row`, `account_detail_lines`,
  `footer_lines`, `cooldown_label`, the plain and rich pool renderers and
  the relay total block. Depends on `render`; called by `cli`.
- **`service.rs`** gains the platform-text parser: `service_status_panel`,
  `parse_relative_seconds`, `unit_seconds` — the module that already owns
  the systemd/launchd unit owns the shape of their status text.
- **`tokens.rs`** (Credential store) gains `load_usage`/`save_usage`: the
  one module that reads or writes an Account's files under the auth dir
  (`cloaking_versions.rs` keeps its own cache there). `AccountManager`
  calls them as free functions, exactly as it calls `save_token`.
- **`cli.rs`** keeps `Args`, `CliRuntime`, `run_with_env`, the verbs, and
  a `CommandEnv { config_path, home, cwd }` value that replaces the three
  repeated parameters (and the `too_many_arguments` allow on `login`).
- `ARCHITECTURE.md` describes the new modules and the two invariants that
  were missing: usage counters persist (cooldown does not), and cloaking
  follows Claude Code except `redact-thinking` (ADR-0014).

## Acceptance criteria

- AC-1: every existing test passes unchanged in what it asserts; tests
  move files only where the function they exercise moved (render width
  tests to `render.rs`, usage-file tests stay in `tests/accounts.rs`).
- AC-2: `cli.rs` contains no `│`/`┌`/`─` panel literals, no ANSI
  constants, and no function that parses the admin payload; `render.rs`
  contains no reference to `Value` payload fields (`totalRequests`,
  `email`, `cooldownUntil`) or to the words Pool/Account.
- AC-3: `service.rs` owns `service_status_panel`; `cli.rs` calls it.
- AC-4: `accounts.rs` performs no `std::fs` call; `usage.json` reads and
  writes go through `tokens::load_usage`/`tokens::save_usage`, which keep
  temp+rename and `0600`.
- AC-5: no `#[allow(clippy::too_many_arguments)]` remains in `cli.rs`;
  `CommandEnv` carries `config_path`, `home`, `cwd`.
- AC-6: byte-identical output: for the fixture set in `tests/cli.rs`,
  plain and rich `status`/`accounts`/`service status`/`login`/`update`/
  `config` outputs equal the pre-refactor outputs (the existing string
  and width assertions are the oracle; no assertion is weakened).
- AC-7: `ARCHITECTURE.md` lists `render.rs`, `usage_view.rs`, the
  service-status parser under Service, `load_usage`/`save_usage` under
  Credential store, and the two new invariants.

## Non-goals

- No behavior change anywhere: no new panels, no wording changes, no
  change to the admin payload, `usage.json` format, or `Style::from_tty`.
- No `CredentialStore` trait: one adapter is a hypothetical seam.
- No touching `utils.rs`, `app.rs` layout, or `AccountManager`'s name.
- No new dependencies.

## Verification

```
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
grep -c '│\|┌\|\\x1b' src/cli.rs          # 0
grep -c 'std::fs' src/accounts.rs          # 0
grep -c 'too_many_arguments' src/cli.rs    # 0
grep -cE 'totalRequests|cooldownUntil|"email"' src/render.rs   # 0
```
