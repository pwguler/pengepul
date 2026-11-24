# model-allowlist

## Goal
Let the operator restrict which models each Provider exposes. A Provider whose `providers:`
entry carries an `allow:` list advertises only the listed models its Upstream lists, and a
request for any other model of that Provider is refused. A Provider without `allow:` behaves
as before: it advertises everything its Upstream lists (automatic-model-coverage).

## Non-goals
- No globs, regexes, substrings or case folding: an entry names one model id exactly.
- No deny list, and no allowlist that spans Providers.
- No fabricated ids: an entry the Upstream does not list is never advertised
  (automatic-model-coverage AC-5).
- No change to how a model id resolves to a Provider (`ModelCatalog::resolve_id`), nor to
  Translation, Cloaking or the name the vendor is told (ADR-0025).
- `/admin/reload` re-reads `allow:` and no other config key.
- No CLI verb that writes `allow:`; it is an edit to `config.yaml`.

## Acceptance criteria
- AC-1: Every `providers:` entry accepts `allow:`, a list of model ids. A built-in Provider
  (`anthropic`, `codex`, `grok`) may have an entry that carries `allow:` and nothing else; a
  built-in entry with a `base-url`, with `models:`, or without `allow:` is refused at load.
  An alias of a built-in (`claude`) is refused as before. `login` and `register_provider`
  keep refusing every built-in name.
- AC-2: `allow: []` is refused at load, with an error that says an empty list would serve
  none of the Provider's models. An `allow:` with no entries (all commented out, or `~`) is
  the same empty list.
- AC-3: `/v1/models` advertises, for a Provider with `allow:`, only the ids in both its
  fetched list and its `allow:`; a Provider without `allow:` advertises its whole fetched list.
- AC-4: An entry matches a model when it equals the model's id without the `<provider>/`
  prefix, byte for byte: `claude-opus-5-5` for anthropic, `deepseek/deepseek-v4.1-flash` for
  a configured `commandcode`.
- AC-5: A request whose model resolves to a Provider with `allow:` and whose id (AC-4) is not
  on the list answers 400 with an error naming the model and the Provider, before an Account
  is chosen, so no Account is charged and nothing is counted. This holds for an explicit
  `<provider>/<model>` id and for a bare id that resolves by a fetched list or by the prefix
  heuristic, on every route that resolves a model: `/v1/chat/completions`, `/v1/responses`,
  `/v1/messages` and `/v1/messages/count_tokens`. An allowed model is served as before. A
  body's `models` list, which OpenRouter serves as fallbacks when `model` fails, names models
  too: on a Provider with `allow:` every entry must be on the list, or the request answers
  the same 400.
- AC-6: Each catalog refresh that fetches a Provider's list logs one warning naming the
  `allow:` entries that list does not contain, so a typo is visible.
- AC-7: `POST /admin/reload` re-reads `allow:` from the config file the relay loaded, and
  `/v1/models` and routing follow the new lists. A config that would not load refuses the
  reload and leaves the lists in force unchanged, and so does a config that drops the entry of
  a configured Provider still serving, whose list would otherwise go with it. The refusal
  quotes nothing from the file; the relay log carries the load error.
- AC-8: `cargo fmt --check`, `cargo clippy --locked --all-targets --all-features -- -D
  warnings` and `cargo test --locked` pass. Tests cover AC-1 to AC-5 and AC-7, and a Provider
  without `allow:` advertising and serving as before, its `models` list included.

## Verification
```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
```

Outside check: a second relay from the branch on a free port, with a copy of the live config
plus `allow:` for `commandcode` and `anthropic`:

```sh
curl -sS localhost:18417/v1/models -H "authorization: Bearer <local-key>" | jq -r '.data[].id'
# only the allowed commandcode and anthropic ids, every other Provider whole
curl -sS localhost:18417/v1/chat/completions -H "authorization: Bearer <local-key>" \
  -H 'content-type: application/json' \
  -d '{"model":"anthropic/<not allowed>","messages":[{"role":"user","content":"hi"}]}'
# 400 naming the model and anthropic; the same request for an allowed id is served
```
