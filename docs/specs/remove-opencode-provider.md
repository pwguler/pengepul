# remove-opencode-provider

## Goal
Remove opencode support from pengepul so the binary serves only anthropic and codex: every opencode code path, credential, model-catalog entry, CLI option, and doc mention is deleted. Leftover opencode credentials on disk are never read, never advertised, and never deleted by the binary — they are ignored until the operator removes them by hand. No opencode-specific rejection logic remains: stale `opencode/...` model ids fail through the generic unknown-model 400 and `--provider opencode` fails at argument parsing.

## Non-goals
- No change to anthropic or codex behavior, routing, cloaking, translation, or the account model.
- No opencode-specific error messages, migration, or cleanup tooling: stale input is ignored or falls through to generic errors, never special-cased; the binary never deletes or migrates leftover opencode credential files.
- No change to the two ADRs' historical *decisions*; only their opencode references are scrubbed so the tree greps clean.
- No README content change beyond removing opencode traces (there are none today; verified by AC-3).

## Acceptance criteria
- AC-1: The codebase compiles and tests green with opencode gone: `cargo build --locked`, `cargo test --locked`, `cargo clippy --locked --all-targets --all-features -- -D warnings`, `cargo fmt --check` all pass.
- AC-2: `grep -rin "opencode" src/ tests/ Cargo.toml README.md CONTEXT.md docs/ scripts/ .github/` returns nothing **except** the test fixtures that exercise the removed provider's rejection paths (the unknown-model assertion in `src/models.rs`, the app/cli rejection tests) — shipped code, docs, and config carry zero opencode references.
- AC-3: `pengepul login --provider opencode` exits with a clap parse error (the value list is `anthropic, codex` only); `pengepul login --key ...` is rejected as an unknown flag.
- AC-4: A request with `"model": "opencode/glm-5.1"` on any route returns 400 `unknown model: opencode/glm-5.1` and never reaches an upstream.
- AC-5: Leftover opencode credentials are inert: with `auth-dir/opencode/` (or legacy flat `opencode-*.json`) present, `GET /admin/accounts` reports only `anthropic` and `codex` providers, startup logs count only anthropic and codex accounts, and the files are left untouched on disk.
- AC-6: `RefreshPolicyKind::Never` and the opencode login import helpers (`save_opencode_login`, `import_opencode_key*`, `opencode_auth_json_paths`, `opencode_key_from_auth_json`) are deleted, not left as dead code.
- AC-7: Tests cover the inert leftovers (AC-5), the unknown-model 400 for a prefixed opencode id (AC-4), and the CLI parse rejection (AC-3).

## Verification
```
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
grep -rin "opencode" src/ tests/ Cargo.toml README.md CONTEXT.md docs/ scripts/ .github/; test $? -eq 1
cargo run -- login --provider opencode; test $? -ne 0
```
