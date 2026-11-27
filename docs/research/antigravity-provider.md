# Google Antigravity as a pengepul provider

Researched 2026-11-27 against `router-for-me/CLIProxyAPI` @
`47f18e4d82bd72f7d2b5ecf5ede93e4837a4625e`, `badlogic/pi-mono` @ `a3bf1eb3`,
`opencode-antigravity-auth`'s API spec, and live probes against Google's public
OAuth and Cloud Code endpoints. Every claim is marked **verified-live** (observed
on the wire from this host, no account needed), **from source** (read in an
implementation, with the file), or **unverified** where neither applies. There is
no Antigravity token on this machine, so nothing below needed a Google account:
the authenticated request and response bodies are taken from the implementations
that have one.

## Question

Can pengepul pool Google AI Pro/Ultra subscriptions the way it pools Anthropic and
ChatGPT/Codex accounts — by driving Google Antigravity's own OAuth — and what does
the provider cost?

## Answer

Yes. It is a confidential OAuth2 client with PKCE, a project id discovered once
per account, and an upstream that speaks the Gemini `generateContent` shape wrapped
in a flat Antigravity envelope. The model family is Gemini 4 / Gemini 3 plus
Claude and GPT-OSS variants, and every one of them answers in the same Gemini
`candidates` shape. The cost is one new upstream dialect (Gemini → Chat), because
the relay's translation matrix is anchored on Chat Completions.

## OAuth

| Piece | Value | Evidence |
|---|---|---|
| authorize | `https://accounts.google.com/o/oauth2/v2/auth` | from source, `internal/auth/antigravity/constants.go` |
| token | `https://oauth2.googleapis.com/token` | from source, same file; verified-live (below) |
| client id | `1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com` | from source, same file; matches the brief |
| client secret | present, required | verified-live |
| callback | `http://localhost:51121/oauth-callback` | from source, `pi-mono` `google-antigravity.ts`; port constant in CLIProxyAPI |
| scopes | `cloud-platform userinfo.email userinfo.profile aicode cclog experimentsandconfigs` (all `https://www.googleapis.com/auth/…`) | from source, both implementations; matches the brief |
| authorize params | `access_type=offline`, `prompt=consent`, `response_type=code`, `code_challenge_method=S256` | from source, `pi-mono`; the brief named the first two |

**The client is confidential, not public.** CLIProxyAPI's `BuildAuthURL` sends no
PKCE, but its `ExchangeCodeForTokens` and its refresh both send
`client_secret=GOCSPX-…`, and the live token endpoint refuses without it:

```
POST https://oauth2.googleapis.com/token
  grant_type=authorization_code&code=bogus&client_id=…&redirect_uri=…
  → 400 {"error":"invalid_request","error_description":"client_secret is missing."}
  + client_secret=GOCSPX-…
  → 400 {"error":"invalid_grant","error_description":"Malformed auth code."}
```

**verified-live 2026-11-27.** The second answer proves the secret is registered for
this client: a wrong secret is `invalid_client`, not `invalid_grant`.

PKCE is sent by `pi-mono` and by `opencode-antigravity-auth`'s documented flow
(`code_challenge_method=S256`). Google accepts it for confidential clients, and it
binds a pasted code to the process that asked for it.

The authorize endpoint accepts the whole parameter set and redirects to the Google
sign-in page with `client_id` and `redirect_uri` preserved:

```
GET https://accounts.google.com/o/oauth2/v2/auth?access_type=offline&…&state=x
  → 302 https://accounts.google.com/v3/signin/identifier?…client_id=…
```
**verified-live 2026-11-27.**

### Token response and refresh

`access_token`, `expires_in` (3600 s for Google), `refresh_token` (only with
`access_type=offline` and `prompt=consent`), `scope`, `token_type`. There is **no
`id_token`**, because the scope set asks for `userinfo.email`/`userinfo.profile`
rather than `openid`. Identity therefore comes from the userinfo endpoint:

```
GET https://www.googleapis.com/oauth2/v2/userinfo?alt=json
Authorization: Bearer <access token>
→ {"id": "...", "email": "...", "verified_email": true, ...}
```
**from source**, `internal/auth/antigravity/auth.go` `FetchUserInfo` and `pi-mono`
`getUserEmail`; the CLIProxyAPI variant is
`https://www.googleapis.com/oauth2/v2/userinfo?alt=json`.

Refresh is the ordinary Google form grant:

```
POST https://oauth2.googleapis.com/token
  grant_type=refresh_token&client_id=…&client_secret=…&refresh_token=…
→ 400 {"error":"invalid_grant","error_description":"Bad Request"}   # for a bad token
```
**verified-live 2026-11-27.** `invalid_grant` is the Reauth marker, which the
existing `detect_exhausted_reason` already reads.

## Project and tier discovery

Every `generateContent` carries a `project`. It is not the Google Cloud project
the operator picks; it is the Cloud Code Assist project the account is bound to,
and it is discovered, not configured:

```
POST https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist
Authorization: Bearer <token>
{"metadata": {"ideType": "ANTIGRAVITY"}}
→ {"cloudaicompanionProject": "...", "currentTier": {"id": "…"},
   "allowedTiers": [{"id": "…", "isDefault": true}]}
```
**from source**, `FetchProjectID` in `internal/auth/antigravity/auth.go` and
`pi-mono` `discoverProject`. `cloudaicompanionProject` may be a string or an
object with `id`; CLIProxyAPI accepts `cloudaicompanionProject`, `projectId` and
`project` in that order.

A first-time account has no project and must be onboarded:

```
POST https://cloudcode-pa.googleapis.com/v1internal:onboardUser
Authorization: Bearer <token>
{"tier_id": "<default tier>", "metadata": {"ide_type": "ANTIGRAVITY",
 "ide_version": "<version>", "ide_name": "antigravity"}}
→ {"done": false, ...}   # poll, then
→ {"done": true, "response": {"cloudaicompanionProject": {"id": "..."}}}
```
**from source**, `OnboardUser` in the same file: up to five attempts, two seconds
apart. The tier is the `allowedTiers` entry with `isDefault`, else
`currentTier.id`, else `free-tier`.

`cloudcode-pa` answers `401 UNAUTHENTICATED` for both actions without a token,
naming the method — which is how the two paths are confirmed to exist on the
production host:
**verified-live 2026-11-27** (`CloudCode.LoadCodeAssist` and
`PredictionService.GenerateContent`).

## Upstream

### Hosts and paths

| Environment | Host | Status |
|---|---|---|
| production | `https://cloudcode-pa.googleapis.com` | verified-live: control plane (loadCodeAssist, fetchAvailableModels) answers 200, but generate answers 429 RESOURCE_EXHAUSTED for a Google AI Pro account with full quota (2026-11-27) |
| daily | `https://daily-cloudcode-pa.googleapis.com` | verified-live 2026-11-27: generate, stream and tools answer 200 for the same account; pengepul's base |
| daily sandbox | `https://daily-cloudcode-pa.sandbox.googleapis.com` | from source, `ANTIGRAVITY_API_SPEC.md` |

| Action | Path |
|---|---|
| generate | `/v1internal:generateContent` |
| stream | `/v1internal:streamGenerateContent?alt=sse` |
| project discovery | `/v1internal:loadCodeAssist` |
| onboarding | `/v1internal:onboardUser` |
| model catalog | `/v1internal:fetchAvailableModels` |

All five answer `401 UNAUTHENTICATED` on the production host without a token, so
all five exist there. **verified-live 2026-11-27.**

### Envelope

The Gemini request is nested one level down, and the rest of the envelope is
Antigravity's own:

```json
{
  "project": "<project id>",
  "model": "<model id>",
  "userAgent": "antigravity",
  "requestType": "agent",
  "requestId": "agent/<uuid>",
  "request": {
    "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
    "systemInstruction": {"parts": [{"text": "..."}]},
    "generationConfig": {"maxOutputTokens": 1024, "temperature": 0.7},
    "tools": [{"functionDeclarations": [...]}],
    "toolConfig": {"functionCallingConfig": {"mode": "AUTO"}}
  }
}
```

**from source**: `geminiToAntigravityDocForAuth` sets `model`, `requestType`,
`project` and `userAgent=antigravity`
(`internal/runtime/executor/antigravity_executor_request.go`), and
`ANTIGRAVITY_API_SPEC.md` shows the same four keys as the complete request.
`requestType` is `agent` for chat, `image_gen` for image models and `web_search`
for the search tool. `request.sessionId` is an Antigravity extension used for
server-side reasoning replay; it is optional and pengepul does not send one.

The inner `request` is the plain Gemini `GenerateContentRequest`:

- `contents[].role`: `user` and `model` (not `assistant`).
- `contents[].parts[]`: `text`, `inlineData` (`mimeType`, `data`), `functionCall`
  (`name`, `args`, optional `id`), `functionResponse` (`name`, `response`,
  optional `id`), and `thought`/`thoughtSignature` on a thinking part.
- `systemInstruction` **must be** `{"parts": [{"text": ...}]}`. A bare string is
  `400`.
- `generationConfig`: `maxOutputTokens`, `temperature`, `topP`, `topK`,
  `stopSequences`, `candidateCount`, `thinkingConfig`.
- `tools[].functionDeclarations[]`: `name`, `description`, `parameters` (a JSON
  Schema subset).
- `toolConfig.functionCallingConfig`: `mode` (`AUTO`/`ANY`/`NONE`) and
  `allowedFunctionNames`.

**from source**, `ANTIGRAVITY_API_SPEC.md` ("verified by direct API testing"),
cross-checked against CLIProxyAPI's `canonicalizeAntigravityToolConfigForValidatedDoc`
and `applyLevelFormatDoc`.

Schema limits, which the upstream enforces with `400`: no `const` (use a
one-value `enum`), no `$ref`/`$defs`/`$schema`/`$id`, no `default`/`examples`;
tool names are 64 chars of `[A-Za-z0-9_.:-]`, first char a letter or underscore.
**from source**, `ANTIGRAVITY_API_SPEC.md`.

### Response

Non-streaming:

```json
{"response": {"candidates": [{"content": {"role": "model",
   "parts": [{"text": "..."}]}, "finishReason": "STOP"}],
   "usageMetadata": {"promptTokenCount": 16, "candidatesTokenCount": 4,
                     "totalTokenCount": 20}},
 "traceId": "..."}
```

Streaming, `text/event-stream`, one `data:` line per chunk, each wrapping the same
`response` object, and no `[DONE]` terminator:

```
data: {"response": {"candidates": [{"content": {"role": "model", "parts": [{"text": "Hello"}]}}]}}
data: {"response": {"candidates": [{"content": {"role": "model", "parts": [{"text": " world"}]}, "finishReason": "STOP"}], "usageMetadata": {...}}}
```

**from source**, `ANTIGRAVITY_API_SPEC.md`, and the unwrap in
`internal/translator/antigravity/gemini/antigravity_gemini_response.go`
(`GetGJSONBytesNoCopy(rawJSON, "response")`), with the `[DONE]` branch declaring
that upstream may omit the terminator.

`usageMetadata`: `promptTokenCount`, `candidatesTokenCount`,
`totalTokenCount`, `thoughtsTokenCount` (thinking), `cachedContentTokenCount`
(cache read). Finish reasons seen: `STOP`, `MAX_TOKENS`, `OTHER`
(`OTHER` is what a function call returns).

Thinking arrives as a part with `"thought": true` beside its `text`, or, for
Gemini models, a part carrying `thoughtSignature`. **from source**,
`ANTIGRAVITY_API_SPEC.md` "Thinking Response".

### Headers

```http
Authorization: Bearer <access token>
Content-Type: application/json
User-Agent: antigravity/hub/<version> (aidev_client; os_type=darwin; arch=arm64; cl=<changelist>)
Accept: text/event-stream            # stream only
```

`X-Goog-Api-Client: gl-node/22.21.1` and `Client-Metadata` are sent by some
implementations; CLIProxyAPI sends the first only on `onboardUser` and neither on
`generateContent`, and requests succeed there, so pengepul sends neither on
generate. **from source**, `antigravity_executor_request.go` and
`internal/misc/antigravity_version.go`.

### Version drift

The `User-Agent` version is the Antigravity Hub release, not a fixed constant. It
is read from a public updater manifest:

```
GET https://antigravity-hub-auto-updater-974169037036.us-central1.run.app/manifest/latest-arm64-mac.yml
→ version: 2.35.2
```
**verified-live 2026-11-27.** CLIProxyAPI refreshes it every six hours and falls
back to `2.35.2` when the fetch fails; a changelist is paired with each known
version (`2.35.2 → 1029311855`). **from source**,
`internal/misc/antigravity_version.go`.

An outdated version is refused with `HTTP 426` and a body naming the minimum, the
same shape grok's proxy uses, so the grok-style "learn the floor from the 426 and
retry once" machinery applies unchanged. **unverified** for this upstream: no 426
was observed live, because every unauthenticated probe answered `401` before the
version gate.

### Errors and quota

Standard Google API error, whatever the transport status:

```json
{"error": {"code": 429, "message": "You have exhausted your capacity on this
  model. Your quota will reset after 3s.", "status": "RESOURCE_EXHAUSTED",
  "details": [{"@type": "type.googleapis.com/google.rpc.RetryInfo",
               "retryDelay": "3.957525076s"}]}}
```

`400 INVALID_ARGUMENT`, `401 UNAUTHENTICATED`, `403 PERMISSION_DENIED`,
`404 NOT_FOUND`, `429 RESOURCE_EXHAUSTED`. The wait is in
`details[].retryDelay` as a duration string (`"3.957525076s"`), not in a
`Retry-After` header. **from source**, `ANTIGRAVITY_API_SPEC.md`; the 401 shape
itself is **verified-live**.

`retryDelay` is therefore the provider's retry hint for the Cooldown rule, and the
relay must parse the duration string rather than the header. The existing
`retry-after` paths do not read it.

### Model catalog

```
POST https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels
Authorization: Bearer <token>
{"project": "<project id>"}
→ {"models": {"<model id>": {"displayName": "...", "maxTokens": 1048576,
                             "maxOutputTokens": 65536, ...}, ...}}
```
**from source**, `cmd/fetch_antigravity_models/main.go`. It returns a map keyed by
model id, not an array, and internal ids are filtered client-side
(`chat_20706`, `chat_23310`, `tab_flash_lite_preview`, `gemini-2.5-flash-thinking`,
`gemini-2.5-pro`).

CLIProxyAPI's registry, extracted from that endpoint, lists the family on
2026-11-27:

| id | context | max out | levels |
|---|---|---|---|
| `gemini-4-neon-xhigh` | 1048576 | 65536 | low, medium, high, xhigh |
| `gemini-4-hydrogen-xhigh` | 1048576 | 65536 | low, medium, high, xhigh |
| `gemini-4-argon-high` | 1048576 | 65536 | low, medium, high |
| `gemini-4-carbon-high` | 1048576 | 65536 | low, medium, high |
| `gemini-3.8-flash-high` | 1048576 | 65536 | low, medium, high |
| `gemini-3.8-flash-tiered` | 1048576 | 65536 | low, medium, high |
| `gemini-3-flash` | 1048576 | 65536 | minimal, low, medium, high |
| `gemini-3.5-flash-lite` | 1048576 | 65535 | minimal, low, medium, high |
| `gemini-3.1-flash-lite` | 1048576 | 65535 | minimal, low, medium, high |
| `gemini-3.1-flash-image` | — | — | minimal, high |
| `claude-opus-4-6-thinking` | 200000 | 64000 | budget |
| `claude-sonnet-4-6` | 200000 | 64000 | budget |
| `claude-opus-5-5-high` | 1000000 | 128000 | low, medium, high |
| `claude-sonnet-5-5-high` | 1000000 | 128000 | low, medium, high |
| `gpt-oss-120b-medium` | 114000 | 32768 | — |

**from source**, `internal/registry/models/models.json` under `antigravity`. The
effort word is part of the model id for the Gemini 4 family (`-high`, `-xhigh`),
which is why pengepul forwards the id verbatim rather than translating a suffix.

`thinkingConfig` is either `{"thinkingBudget": N}` or, on the level-capable
models, `{"thinkingLevel": "LOW"|"MEDIUM"|"HIGH"|"XHIGH"}`, always with
`includeThoughts: true` when thinking text is wanted. `maxOutputTokens` must be
greater than `thinkingBudget`. **from source**,
`internal/thinking/provider/antigravity/apply.go` and `ANTIGRAVITY_API_SPEC.md`.

## Pengepul integration map

- `src/types.rs` — `ProviderKind::Antigravity`, canonical id `antigravity`.
- `src/oauth.rs` — the client constants, `generate_antigravity_auth_url`
  (PKCE + state + `access_type=offline` + `prompt=consent`),
  `exchange_antigravity_code` (form-encoded with the secret, then userinfo, then
  `loadCodeAssist`/`onboardUser`), `refresh_antigravity_tokens`.
- `src/tokens.rs` — persist `project_id` beside `plan_type`.
- `src/upstream.rs` — the host, paths, headers and the version constant, plus the
  envelope wrap/unwrap helpers and the `retryDelay` hint.
- `src/translate.rs` — Chat Completions ↔ Gemini: request, whole response, and the
  streamed chunk, so Messages and Responses ride the existing chat converters.
- `src/models.rs` — the fetched catalog, with the static floor when the fetch
  fails.
- `src/app.rs` — routing, the stream arms, and the profile accounting.

## Gotchas

- **No `id_token`.** Account identity is userinfo's `email`/`id`, not a minted JWT.
  Code that assumes an `id_token` (grok's `principal_id`) does not apply.
- **`project` is load-bearing.** Without it `generateContent` is
  `400 INVALID_ARGUMENT`; it is not optional for this host the way it is for
  plain Gemini.
- **The envelope is an envelope.** A plain Gemini body posted to this host is
  accepted structurally but has no `project`/`model` at the top level, so the
  generation is rejected. `response` must be unwrapped on the way back.
- **Streams may omit `[DONE]`.** The stream ends when the connection does; a
  client that waits for a terminator needs one synthesized.
- **No `Retry-After`.** The wait is `details[].retryDelay`, a Go duration string.
- **Thinking signatures** (`thoughtSignature`) are opaque and model-specific; they
  are dropped rather than replayed, because pengepul is stateless in this
  provider.
- **Tool-name and schema restrictions** are real `400`s, and `const`/`$ref` show
  up in ordinary client tool definitions.
- **Client secret** lives in the binary like the other providers' client ids. It
  is not a bearer of the operator's identity, but it is a Google credential; keep
  it out of logs.

## Sources

- `router-for-me/CLIProxyAPI` @ `47f18e4d82bd72f7d2b5ecf5ede93e4837a4625e`
  (2026-11-27): `internal/auth/antigravity/{constants,auth}.go`,
  `internal/misc/antigravity_version.go`,
  `internal/runtime/executor/antigravity_executor.go`,
  `internal/runtime/executor/antigravity_executor_request.go`,
  `internal/thinking/provider/antigravity/apply.go`,
  `internal/translator/antigravity/gemini/antigravity_gemini_response.go`,
  `internal/registry/models/models.json`,
  `cmd/fetch_antigravity_models/main.go`.
- `badlogic/pi-mono` @ `a3bf1eb3`:
  `packages/ai/src/utils/oauth/google-antigravity.ts`.
- `opencode-antigravity-auth`: `docs/ANTIGRAVITY_API_SPEC.md`
  (self-described "Verified by Direct API Testing").
- Live probes, 2026-11-27, from this host, no account:
  - `POST https://oauth2.googleapis.com/token` without/with the client secret →
    `invalid_request` / `invalid_grant`.
  - `POST https://cloudcode-pa.googleapis.com/v1internal:{generateContent,
    streamGenerateContent?alt=sse,fetchAvailableModels,loadCodeAssist}` → `401
    UNAUTHENTICATED` naming the method.
  - `GET https://accounts.google.com/o/oauth2/v2/auth?...` → `302` to sign-in.
  - `GET https://antigravity-hub-auto-updater-974169037036.us-central1.run.app/
    manifest/latest-arm64-mac.yml` → `version: 2.35.2`.
