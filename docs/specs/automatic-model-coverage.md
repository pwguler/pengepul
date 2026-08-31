# automatic-model-coverage

## Goal
Stop hardcoding model catalogs. pengepul fetches each logged-in provider's own model list
from its upstream and uses it as the single source of truth for `/v1/models` (what it
advertises) and for routing (an id the upstream lists routes to that provider). A new
vendor model is covered the moment the vendor lists it, with no code change. Covers
anthropic and codex. The `opus`/`sonnet`/`haiku` aliases are removed.

## Gate results (probed against the live relay's accounts — both confirmed 200)
Exact fetch shapes for implementation:
- **Anthropic**: `GET https://api.anthropic.com/v1/models`, headers `authorization: Bearer
  <access_token>`, `anthropic-version: 2023-06-01`, `anthropic-beta: oauth-2025-04-20`.
  Response `{"data": [{"id": "..."}]}`. Probed: 200, 10 models.
- **Codex**: `GET https://chatgpt.com/backend-api/codex/models?client_version=<cli-version>`
  (the query param is required — a 400 `missing client_version` without it), headers
  `authorization: Bearer <access_token>`, `originator: codex_cli_rs`, `version:
  <cli-version>`, `ChatGPT-Account-ID: <account_uuid>`. Response `{"models": [{"slug":
  "gpt-5.5", "display_name": ...}]}` — the model id is `slug`, not `id`. Probed with a
  fresh token: 200, 3 models. Reuse the config cli-version (default `0.125.0`).

## Non-goals
- No per-request upstream fetch. Lists are cached; the hot path reads memory.
- Do not keep the `opus`/`sonnet`/`haiku` aliases or any hardcoded advertising list.
- No change to how a matched request is translated or cloaked.

## Acceptance criteria
- AC-1: `/v1/models` returns the union of models fetched live from each logged-in
  provider's upstream (anthropic `/v1/models`, codex `/codex/models`), `owned_by` set to
  the provider. No advertising list is hardcoded.
- AC-2: Routing resolves a model id by which provider's fetched list contains it: an id in
  the Anthropic list routes to Anthropic, Codex list to Codex.
- AC-3: Lists are fetched into a cache with a TTL and refreshed off the hot path;
  `/admin/reload` also refreshes them. A request never triggers a synchronous fetch.
- AC-4: Cache-miss fallback: an id in no cached list routes by prefix heuristic
  (`claude-*`/`anthropic*` -> Anthropic; `gpt-*`/`o<N>`/`codex-*` -> Codex); an id
  matching no heuristic returns a 400 unknown-model error rather than silently routing.
- AC-5: On upstream fetch failure, the last good cache is kept and the failure is logged; a
  provider that never fetched successfully contributes nothing to `/v1/models` and falls
  back to the prefix heuristic for routing. No stale or fabricated ids are served silently.
- AC-6: The `opus`/`sonnet`/`haiku` aliases and `MODEL_ALIASES` are gone; those names are
  treated as ordinary ids per AC-4.
- AC-7: `cargo fmt --check`, `cargo clippy --locked --all-targets --all-features -- -D
  warnings`, and `cargo test --locked` pass. New tests cover list-based routing, the
  cache-miss heuristic, the unknown-model 400, and the fetch-failure fallback.

## Verification
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
# with a logged-in relay:
curl -sS localhost:8317/v1/models -H "authorization: Bearer <local-key>" | jq '.data[].id'
