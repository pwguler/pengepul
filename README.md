# pengepul

Run your own API relay for your AI subscriptions. Log in your Claude and ChatGPT/Codex
accounts once; pengepul serves every request from the pool, so your harness runs on your
subscription instead of a per-token API key.

- Pools several subscription accounts per provider and spreads requests across them.
- Serves your subscription inside openclaw and hermes, with no API key.
- Relays any OpenAI-compatible API (groq, openrouter, deepseek, ...) through the same pool.
- Exposes the pool as a REST API, local or networked, for your own tools.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/pwguler/pengepul/main/scripts/install.sh | sh
```

Linux x86_64 and macOS on Apple silicon. `pengepul update` installs the most recent
release (`--check` reports it without installing); both verify the published checksum.
From source: `cargo install --git https://github.com/pwguler/pengepul.git --locked`.

## Quickstart

```sh
pengepul login # authorize an Anthropic account
pengepul login --provider codex # authorize a ChatGPT/Codex account
pengepul serve # binds 127.0.0.1:8317
pengepul serve --host 0.0.0.0 --port 8317 # reachable across your network
```

Log in more than once per provider to pool several accounts; requests round-robin across
them. Credentials live under `~/.pengepul` (`0600`); a running relay picks up a fresh
login on restart or `pengepul accounts --reload`. Read the local API key clients use from
`pengepul config api-key`. Exposed on a network, that key is the only thing guarding your
pooled subscriptions, so keep it secret and prefer a trusted network or an SSH tunnel.

### OpenAI-compatible endpoints

Point the pool at any service that speaks the OpenAI API. Add a `providers:` entry to
`~/.pengepul/config.yaml` (one per endpoint), then save its static API key; requests
address its models with a `<provider>/<model>` prefix:

```sh
# ~/.pengepul/config.yaml
providers:
  groq:
    base-url: https://api.groq.com/openai/v1
  openrouter:
    base-url: https://openrouter.ai/api/v1
```

```sh
pengepul login --provider groq --key $GROQ_API_KEY # save a key (repeat to pool more)
curl -sS http://127.0.0.1:8317/v1/chat/completions \
  -H "Authorization: Bearer $API_KEY" -H "Content-Type: application/json" -d '{
  "model": "groq/llama-3.3-70b-versatile",
  "messages": [{"role": "user", "content": "reply exactly: pong"}]}'
```

Configured endpoints accept the Chat Completions dialect and rotate across their keys
with the same failure handling as the subscription providers.

Login opens a browser and completes on a localhost callback. On a remote host, forward the
callback port first:

```sh
ssh -L 54545:localhost:54545 user@host # anthropic
ssh -L 1455:localhost:1455 user@host # codex
```

## Clients

### openclaw

The embedded runner talks native Anthropic Messages. In `~/.openclaw/openclaw.json`,
register a `pengepul` provider and select it with a `pengepul/`-prefixed model. A bare
`claude-…` resolves to the claude-cli backend and bypasses pengepul:

```json
{
  "agents": { "defaults": { "model": { "primary": "pengepul/claude-opus-5" } } },
  "models": {
    "providers": {
      "pengepul": {
        "baseUrl": "http://127.0.0.1:8317",
        "apiKey": "<pengepul api-key>",
        "auth": "api-key",
        "models": [
          { "id": "claude-opus-5", "name": "Claude Opus 5", "api": "anthropic-messages", "contextWindow": 1000000, "maxTokens": 64000 }
        ]
      }
    }
  }
}
```

### hermes

Register pengepul as a named provider on the native Messages wire, in
`HERMES_HOME/config.yaml`:

```sh
hermes config set model.provider pengepul
hermes config set model.default claude-opus-5
hermes config set providers.pengepul.base_url http://127.0.0.1:8317
hermes config set providers.pengepul.api_mode anthropic_messages
hermes config set providers.pengepul.api_key <pengepul api-key>
```

- `api_mode: anthropic_messages` forces the native wire. The `base_url` may be the root
  or end in `/v1`; both work.
- Use `provider: pengepul`, not `anthropic`. An `anthropic` provider makes hermes
  autodiscover the operator's `~/.claude` OAuth and route to `api.anthropic.com`,
  bypassing pengepul.
- Rotating `providers.*.api_key` in an existing home caches the old key's rejection in
  `auth.json`; use a fresh home or delete `auth.json`.

### Your own harness

pengepul is a plain REST relay, so any client that speaks the Anthropic or OpenAI API can
run on the pool. Point it at `http://127.0.0.1:8317/v1` with the local API key; a root
base URL without `/v1` works as well.

```sh
# Claude, on the Anthropic Messages API
curl -sS http://127.0.0.1:8317/v1/messages \
  -H "Authorization: Bearer $API_KEY" -H "Content-Type: application/json" -d '{
  "model": "claude-opus-5",
  "max_tokens": 128,
  "messages": [{"role": "user", "content": "reply exactly: pong"}]}'

# Codex, on the OpenAI Chat Completions API (/v1/responses works too)
curl -sS http://127.0.0.1:8317/v1/chat/completions \
  -H "Authorization: Bearer $API_KEY" -H "Content-Type: application/json" -d '{
  "model": "gpt-5.4",
  "messages": [{"role": "user", "content": "reply exactly: pong"}]}'

# groq, through a configured provider
curl -sS http://127.0.0.1:8317/v1/chat/completions \
  -H "Authorization: Bearer $API_KEY" -H "Content-Type: application/json" -d '{
  "model": "groq/llama-3.3-70b-versatile",
  "messages": [{"role": "user", "content": "reply exactly: pong"}]}'
```

## Commands

```sh
pengepul serve # start the relay (the default with no subcommand)
pengepul login # authorize an account in a browser (--provider codex for Codex)
pengepul login --provider groq --key $KEY # save a static key for a configured provider
pengepul status # health of the running relay
pengepul accounts # loaded accounts (--reload re-reads from disk)
pengepul update # install the most recent release (--check only reports)
pengepul config path|show|api-key # show the config path, contents, or a key
pengepul service install|start|stop|restart|status|uninstall|logs # manage the user service (systemd on Linux, launchd on macOS)
```

Run `pengepul <command> --help` for flags. The service is user-scoped, so
`systemctl status pengepul` will not find it; use `pengepul service status`, or add
`--user`.

## Reference

Routes: `POST /v1/messages`, `POST /v1/chat/completions`, `POST /v1/responses`,
`POST /v1/messages/count_tokens`, `GET /v1/models`, `GET /admin/accounts`,
`POST /admin/reload`, and `GET /health` (unauthenticated). Every route but `/health` needs
the local API key, as either `Authorization: Bearer <key>` or `x-api-key: <key>`.

The provider is chosen by model id: `gpt-5`, `gpt-5.*`, `gpt-5-*`, `o<N>` and `codex-*`
route to Codex, `claude-*` to Anthropic, and `<id>/<model>` routes to the configured
provider `id` (`groq/llama-3.3-70b-versatile`). A request with no `model` is rejected
with 400, as is a prefix no configured provider claims. `count_tokens` and the Messages
and Responses routes answer 501 for configured providers; they accept only Chat
Completions.

pengepul writes `~/.pengepul/config.yaml` when it is missing, generating a fresh
`sk-local-…` key. The keys you can set:

```yaml
host: '' # empty binds 127.0.0.1, not every interface
port: 8317
auth-dir: ~/.pengepul
api-keys:
  - sk-local-example
providers:
  groq:
    base-url: https://api.groq.com/openai/v1
body-limit: 200mb # checked against Content-Length; empty means unlimited
timeouts:
  messages-ms: 120000
  stream-messages-ms: 600000
  count-tokens-ms: 30000
debug: off # off | errors | verbose
```

The CLI versions pengepul presents upstream follow what Claude Code and Codex currently
ship: the relay reads the npm registry and the Codex GitHub releases once a day, keeps the
result in `<auth-dir>/cloaking-versions.json`, and uses it on the next request without a
restart. `cloaking.cli-version` and `cloaking.codex.cli-version` are optional floors: set
one only to hold a version newer than the published one.

Requests round-robin across accounts with no session affinity, failing over once per
account on upstream 401, 403, 429, 500 and 502-599. Failover never crosses providers: a
request stays on the endpoint or subscription family it named. A failed account backs
off up to 5 minutes; a dead refresh token locks it out for 24 hours until a fresh
`pengepul login`.
