# grok-provider

## Goal

pengepul pools grok.com subscriptions like it pools Anthropic and ChatGPT/Codex
accounts: `pengepul login --provider grok` authorizes accounts through grok
build's own OAuth, and the relay serves grok models off that pool on every
inbound dialect. Protocol facts and sources live in
`docs/research/grok-build-provider.md`.

## Non-goals

- No device-code login and no import of the grok CLI's `~/.grok/auth.json` —
  refresh tokens there are single-use, so sharing an identity between the CLI
  and pengepul races into Reauth.
- No Responses-API upstream dialect. The proxy's chat-completions endpoint is
  the only upstream shape; it carries `reasoning_effort` and streams (verified
  live), so Responses adds surface, not reach.
- No usage accounting for the proxy's `cost_in_usd_ticks` field (tick scale
  unverified).
- No config knob for the client version — the 426 auto-learn covers drift.
- No first-class keyed xAI provider: the generic configured-endpoint path
  (`--provider xai --base-url https://api.x.ai/v1 --key …`) already serves it.
- No change to anthropic, codex, or configured-provider behavior, metadata, or
  rotation.

## Acceptance criteria

- AC-1: `pengepul login --provider grok` runs the auth.x.ai authorization-code
  flow with PKCE (`https://auth.x.ai`, client id
  `b1a00492-073a-47ea-816f-4c329264a828`, scopes `openid profile email
  offline_access grok-cli:access`, `referrer=grok-build`), opens the browser,
  binds a fixed localhost callback distinct from the other providers', and
  stores an account labeled with the login's email. Logging in again pools a
  second account; `--key` and `--base-url` are refused for it, like the other
  built-ins.
- AC-2: `GET /v1/models` advertises `grok/grok-4.6` and `grok/grok-4.5` (500k
  context). Requests may address them bare (`grok-4.6`) or prefixed
  (`grok/grok-4.6`); both route to the grok pool, and `xai/…` still routes to
  a configured provider named `xai`.
- AC-3: A Chat Completions request naming a grok model is answered in standard
  OpenAI shape, streaming and non-streaming, with the upstream
  `reasoning_content` (field or delta) passed through untouched, and usage
  reported. `reasoning_effort` in the client body reaches the upstream.
- AC-4: An Anthropic-messages or OpenAI-responses request naming a grok model
  is translated to the chat dialect upstream and answered in the inbound
  dialect, by the existing translation matrix. Grok's `reasoning_content` is
  dropped in that translation — it passes through only on the Chat
  Completions dialect (AC-3), never as a thinking block on the others.
- AC-5: Every upstream request carries `Authorization: Bearer <session
  token>`, `X-XAI-Token-Auth: xai-grok-cli`, and `x-grok-client-version`
  (default `0.1.202`), against `https://cli-chat-proxy.grok.com/v1`.
- AC-6: A 426 whose body names a minimum version ("version X or later") is
  retried once with that version, which stays adopted for the process
  lifetime; the client sees the retried result. A 426 without a parsable
  minimum surfaces as an upstream error.
- AC-7: Account lifecycle reuses the existing machinery: an expired access
  token refreshes from its stored refresh token without operator involvement,
  and a rejected refresh token lands the account in Reauth, removing it from
  rotation until a fresh login.
- AC-8: Account identity comes from the minted id_token: email, account id,
  and the plan/tier label shown by `pengepul accounts` / status.

## Verification

```sh
cargo test
cargo test grok
```

Manual smoke, after a real `pengepul login --provider grok` and
`pengepul serve`:

```sh
KEY=$(pengepul config api-key)
curl -sS http://127.0.0.1:8317/v1/chat/completions -H "Authorization: Bearer $KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"grok-4.6","messages":[{"role":"user","content":"reply exactly: pong"}]}'
curl -sS -N http://127.0.0.1:8317/v1/chat/completions -H "Authorization: Bearer $KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"grok-4.6","stream":true,"messages":[{"role":"user","content":"reply exactly: pong"}]}'
curl -sS http://127.0.0.1:8317/v1/messages -H "Authorization: Bearer $KEY" \
  -H 'Content-Type: application/json' -H 'anthropic-version: 2023-06-01' \
  -d '{"model":"grok-4.6","max_tokens":64,"messages":[{"role":"user","content":"reply exactly: pong"}]}'
```

The first returns a chat.completion with content, the second a stream of
chat.completion.chunks ending in `[DONE]`, the third an Anthropic message —
all served from the grok pool.
