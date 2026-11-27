# antigravity-provider

## Goal

pengepul pools Google AI Pro/Ultra subscriptions like it pools Anthropic and
ChatGPT/Codex accounts: `pengepul login --provider antigravity` authorizes an
account through Google Antigravity's own OAuth, discovers the Cloud Code Assist
project the account is bound to, and the relay serves the Gemini 4 family off that
pool on every inbound dialect. Protocol facts and sources live in
`docs/research/antigravity-provider.md`.

## Non-goals

- No image models (`requestType: image_gen`) and no `google_search` tool. Both are
  separate code paths upstream and neither is a coding-harness model.
- No server-side reasoning replay: `request.sessionId` and `thoughtSignature` are
  not stored or replayed, so reasoning is not preserved across turns.
- No `onboardUser` for an account that already has a project, and no shared
  fallback project id.
- No config knob for the client version or the host: the version is learned from
  the 426 and refreshed from the public manifest, the host is a constant.
- No change to anthropic, codex, grok or configured-provider behaviour, metadata,
  or rotation.

## Acceptance criteria

- AC-1: `pengepul login --provider antigravity` runs Google's authorization-code
  flow with PKCE against `https://accounts.google.com/o/oauth2/v2/auth` (client id
  `1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com`,
  client secret sent on exchange and refresh, redirect
  `http://localhost:51121/oauth-callback`, scopes `cloud-platform
  userinfo.email userinfo.profile aicode cclog experimentsandconfigs`,
  `access_type=offline`, `prompt=consent`), prints the URL, binds the loopback
  callback, and stores an account labeled with the userinfo email, carrying the
  discovered project id and the tier label. A headless host accepts the code or
  the full callback URL pasted on stdin, whichever of the two arrives first.
  `--key` and `--base-url` are refused for it, like the other built-ins.
- AC-2: `GET /v1/models` advertises `antigravity/gemini-4-neon` and its effort
  tiers `antigravity/gemini-4-neon-low`, `-medium`, `-high`, `-xhigh` when the
  live catalog has not been fetched, and the ids the upstream lists when it has.
  Requests may address them bare (`gemini-4-neon`) or prefixed
  (`antigravity/gemini-4-neon`); both route to the antigravity pool, and the id
  reaches the vendor as the client wrote it (ADR-0025).
- AC-3: A Chat Completions request naming an antigravity model is answered in
  standard OpenAI shape, streaming and non-streaming, with reasoning delivered as
  `reasoning_content` and usage reported from `usageMetadata`. `reasoning_effort`
  in the client body reaches the upstream as `generationConfig.thinkingConfig`,
  and `maxOutputTokens` is raised above the thinking budget when the client set a
  smaller one.
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
  results, system instructions and images are carried in their Gemini shapes.
- AC-6: A 426 whose body names a minimum version ("version X or later") is retried
  once with that version, which stays adopted for the process lifetime; the client
  sees the retried result. The `User-Agent` version is otherwise refreshed from the
  public Antigravity Hub manifest once per process, falling back to the pinned
  constant when that fetch fails.
- AC-7: Account lifecycle reuses the existing machinery: an expired access token
  refreshes from its stored refresh token without operator involvement and keeps
  its stored project id, and a rejected refresh token (`invalid_grant`) lands the
  account in Reauth, removing it from rotation until a fresh login.
- AC-8: A `429` carrying `details[].retryDelay` ("3.957525076s") sets the Cooldown
  through the existing Retry-hint rule, and no upstream header is passed to the
  client.

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
  -d '{"model":"gemini-4-neon","messages":[{"role":"user","content":"reply exactly: pong"}]}'
curl -sS -N http://127.0.0.1:8317/v1/chat/completions -H "Authorization: Bearer $KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"gemini-4-neon-high","stream":true,"messages":[{"role":"user","content":"reply exactly: pong"}]}'
curl -sS http://127.0.0.1:8317/v1/messages -H "Authorization: Bearer $KEY" \
  -H 'Content-Type: application/json' -H 'anthropic-version: 2023-06-01' \
  -d '{"model":"gemini-4-neon","max_tokens":64,"messages":[{"role":"user","content":"reply exactly: pong"}]}'
curl -sS http://127.0.0.1:8317/v1/responses -H "Authorization: Bearer $KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"gemini-4-neon","input":"reply exactly: pong"}'
```

The first returns a chat.completion with content, the second a stream of
chat.completion.chunks ending in `[DONE]`, the third an Anthropic message, the
fourth a Responses object — all served from the antigravity pool.
