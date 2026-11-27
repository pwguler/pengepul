# antigravity-provider

## Goal

pengepul pools Google AI Pro/Ultra subscriptions like it pools Anthropic and
ChatGPT/Codex accounts: `pengepul login --provider antigravity` authorizes an
account through Google Antigravity's own OAuth, discovers the Cloud Code Assist
project the account is bound to, and the relay serves the Antigravity catalog off
that pool on every inbound dialect, tool loops included. Protocol facts and
sources live in `docs/research/antigravity-provider.md`.

## Non-goals

- No image models (`requestType: image_gen`) and no `google_search` tool. Both are
  separate code paths upstream and neither is a coding-harness model.
- No server-side session state: `request.sessionId` is not sent, and the relay
  stores no conversation. Thought signatures are not stored either; they travel
  with the conversation the client already sends (AC-9).
- No `onboardUser` for an account that already has a project, and no shared
  fallback project id.
- No config knob for the client version or the host: the version is learned from
  the 426 and refreshed from the public manifest, the host is a constant.
- No change to anthropic, codex, grok or configured-provider behaviour, metadata,
  routing or rotation, and no network call for an install with no antigravity
  account.

## Acceptance criteria

- AC-1: `pengepul login --provider antigravity` runs Google's authorization-code
  flow with PKCE against `https://accounts.google.com/o/oauth2/v2/auth` (client id
  `1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com`,
  client secret sent on exchange and refresh, redirect
  `http://localhost:51121/oauth-callback`, scopes `cloud-platform
  userinfo.email userinfo.profile aicode cclog experimentsandconfigs`,
  `access_type=offline`, `prompt=consent`), prints the URL, binds the loopback
  callback, and stores an account labeled with the userinfo email, carrying the
  discovered project id and the plan label (`paidTier` when the account has one).
  A headless host accepts the full callback URL (copied from the browser's address
  bar after the redirect fails) or its code pasted on stdin, whichever arrives
  first; a pasted URL whose `state` is missing or does not match is refused. `--key` and
  `--base-url` are refused for it, like the other built-ins. `pengepul accounts
  --reload` picks up a fresh login without a restart.
- AC-2: `GET /v1/models` advertises the ids the upstream's `fetchAvailableModels`
  lists, prefixed `antigravity/`, leaving out the internal and non-chat ids
  (`tab_*`, `chat_*`, image models). Before a live list arrives the floor is the
  Gemini 4 Neon effort tiers `gemini-4-neon-low`, `-medium`, `-high`, `-xhigh`;
  the bare `gemini-4-neon` is not an upstream id and is not advertised. A failed
  fetch keeps the last list. Requests may address an id bare or prefixed; the id
  reaches the vendor as the client wrote it (ADR-0025). A bare id that another
  built-in also serves stays with that built-in, and the antigravity copy is
  reachable only with its prefix. A bare id the name heuristic gives to an
  earlier built-in (`claude-*` to anthropic) stays with it while its pool has an
  account, even when its list is missing; an antigravity-only `claude-*` id
  then needs its `antigravity/` prefix.
- AC-3: A Chat Completions request naming an antigravity model is answered in
  standard OpenAI shape, streaming and non-streaming, with reasoning delivered as
  `reasoning_content` and usage reported from `usageMetadata`, thinking counted
  inside `completion_tokens`. `reasoning_effort` reaches the upstream as
  `generationConfig.thinkingConfig` (`none` sends none); `max_completion_tokens`,
  else `max_tokens`, sets `maxOutputTokens`, raised above the thinking budget only
  when the client set a smaller cap. A reply with a function call finishes
  `tool_calls`, streamed or not. A streamed Chat reply ends `[DONE]` only when the
  upstream finished it, and a streamed Messages reply ends `message_stop` only
  then too, an `error` event otherwise; an in-band error frame reaches the
  client as an error.
- AC-4: An Anthropic-Messages or OpenAI-Responses request naming an antigravity
  model is translated onto the Gemini upstream and answered in the inbound
  dialect by the existing translation matrix, exactly as a grok request is:
  reasoning becomes a thinking block on Messages and a reasoning item on
  Responses, and nothing is invented when it is absent. No antigravity inbound
  dialect answers 501.
- AC-5: Every upstream request carries `Authorization: Bearer <access token>`,
  `Content-Type: application/json` and a `User-Agent` of
  `antigravity/hub/<version> (aidev_client; …)`, and its body is the Antigravity
  envelope — `project`, `model`, `userAgent`, `requestType` and the Gemini
  `request` — to `https://daily-cloudcode-pa.googleapis.com/v1internal:generateContent`
  or `…/v1internal:streamGenerateContent?alt=sse`. Tool declarations, tool
  results, system instructions and images are carried in their Gemini shapes;
  tool schemas are reduced to what the Gemini Schema proto accepts (nullable type
  arrays, exclusive bounds, local `$ref`s inlined to any depth with a cycle
  ending as an open object, an array given `items`, and a non-string enum
  keeping its type with its members moved into the description).
- AC-6: A 426 whose body names a minimum version ("version X or later") is retried
  once with that version, which stays adopted for the process lifetime and is
  never lowered; the client sees the retried result. The `User-Agent` version is
  otherwise refreshed from the public Antigravity Hub manifest while the pool has
  an account and at login, falling back to the pinned constant when that fetch
  fails. The read runs in the background with a 3 s limit: neither a catalog
  refresh nor the login URL waits on it.
- AC-7: Account lifecycle reuses the existing machinery: an expired access token
  refreshes from its stored refresh token without operator involvement and keeps
  its stored project id, and a rejected refresh token (`invalid_grant`) lands the
  account in Reauth, removing it from rotation until a fresh login.
- AC-8: A `429` carrying `details[].retryDelay` ("3.957525076s") sets the Cooldown
  through the existing Retry-hint rule when no header names a wait, and no
  upstream header is passed to the client.
- AC-9: A tool loop survives its second turn on Gemini and on Claude models. The
  vendor's call id travels on `functionCall.id` and `functionResponse.id`. A
  call's `thoughtSignature` is carried inside the tool-call id the client is
  given (the vendor id, `__ags__`, then the signature in base64url), so the client
  sends it back with the conversation and the relay replays it on the call. A
  model turn with no signature to replay sends Google's documented
  `skip_thought_signature_validator` on its first call.

## Verification

```sh
cargo test
cargo test antigravity
```

Manual smoke, after a real `pengepul login --provider antigravity` and
`pengepul serve`:

```sh
KEY=$(pengepul config api-key)
curl -sS http://127.0.0.1:8317/v1/chat/completions -H "Authorization: Bearer $KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"gemini-4-neon-medium","messages":[{"role":"user","content":"reply exactly: pong"}]}'
curl -sS -N http://127.0.0.1:8317/v1/chat/completions -H "Authorization: Bearer $KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"gemini-4-neon-high","stream":true,"messages":[{"role":"user","content":"reply exactly: pong"}]}'
curl -sS http://127.0.0.1:8317/v1/messages -H "Authorization: Bearer $KEY" \
  -H 'Content-Type: application/json' -H 'anthropic-version: 2023-06-01' \
  -d '{"model":"gemini-4-neon-medium","max_tokens":64,"messages":[{"role":"user","content":"reply exactly: pong"}]}'
curl -sS http://127.0.0.1:8317/v1/responses -H "Authorization: Bearer $KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"gemini-4-neon-medium","input":"reply exactly: pong"}'
```

The first returns a chat.completion with content, the second a stream of
chat.completion.chunks ending in `[DONE]`, the third an Anthropic message, the
fourth a Responses object — all served from the antigravity pool.

A tool loop (AC-9): send a request with one tool and a prompt that must call it,
then send the conversation back with the returned `tool_calls` (ids unchanged)
and a `tool` result. The second request answers 200 with text, on
`gemini-4-neon-medium` and on `claude-sonnet-5-5-low`, through Chat and Messages,
streamed and not.
