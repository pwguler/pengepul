# Cloaking versions track the vendor CLIs

## Goal

The CLI versions pengepul presents upstream stay current without a release or a config
edit. The relay learns the latest Claude Code and Codex CLI versions from their public
registries, keeps them on disk, and uses them for Cloaking.

## Non-goals

- `X-Stainless-Package-Version` stays hardcoded; there is no reliable public source.
- No install-date model. A vendor CLI auto-updates, so a plausible client is on latest.
- No change to what Cloaking writes; only the version strings it interpolates.

## Design

**Sources.**

| version | source | field |
|---|---|---|
| claude `cli-version` | `GET https://registry.npmjs.org/@anthropic-ai/claude-code` | `dist-tags.latest` |
| codex `cli-version` | `GET https://api.github.com/repos/openai/codex/releases/latest` | `tag_name`, `rust-v` prefix stripped |

**Effective version** for each CLI is `max(configured, cached, baked default)` under semver
ordering. `cloaking.cli-version` and `cloaking.codex.cli-version` become optional floors
rather than fixed values; a config that still pins an old version is never wrong, only
redundant. The baked defaults (`2.1.88`, `0.125.0`) are the last floor.

**Refresh loop.** One task spawned next to the model catalog refresh, inside `serve`.
The server listens first; the first fetch runs immediately after. On success the loop
sleeps 24h. On failure it logs `warn` and retries after 1h. Only a value that parses as
semver is accepted; a bad body leaves the current value in place. A change logs one
`info` line; no change logs nothing.

**Cache.** `<auth-dir>/cloaking-versions.json`:

```json
{ "claude": "2.1.251", "codex": "0.151.0", "fetched-at": "2026-08-30T02:10:00Z" }
```

Written atomically (temp file then rename). Read once at startup so an offline restart
keeps the last known versions. A missing or unreadable file is ignored.

**Requests** read the effective versions from shared state at request time, so a
refresh takes effect without a restart.

## Acceptance criteria

- AC-1 A request cloaked after a successful fetch carries the fetched claude version in
  `User-Agent` and the billing block, and the fetched codex version in codex headers.
- AC-2 With `cloaking.cli-version` set higher than the fetched value, the configured value
  is used; set lower, the fetched value is used. Same for `cloaking.codex.cli-version`.
- AC-3 With both `cli-version` keys absent from config.yaml, config loads and the baked
  defaults apply until a fetch succeeds.
- AC-4 A fetch that fails, or returns a non-semver value, leaves the effective versions
  unchanged and does not stop the server.
- AC-5 After a successful fetch, `<auth-dir>/cloaking-versions.json` holds both versions;
  on the next start, before any fetch, those versions are effective.
- AC-6 A codex `tag_name` of `rust-v0.151.0` becomes `0.151.0`.
- AC-7 `serve` accepts connections before the first fetch completes.

## Verification

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Live check, after `pengepul serve`:

```sh
cat ~/.pengepul/cloaking-versions.json
pengepul serve --debug 2>&1 | grep -m1 'claude-cli/'
```
