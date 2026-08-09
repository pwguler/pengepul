# pengepul

Run your own API relay for your AI subscriptions. Log in your Claude and ChatGPT/Codex
accounts once, and pengepul serves every request from the pool, so your own harness runs
on your subscription instead of a per-token API key.

What it does:

- Pools several subscription accounts per provider and spreads requests across them.
- Keeps your subscription working inside openclaw and hermes, with no API key.
- Exposes the pool as a REST API, local or over your network, so you build your own
  harness with no API key.

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

Exposed on a network, the local API key is the only thing guarding your pooled
subscriptions, so keep it secret and prefer a trusted network or an SSH tunnel.

Login opens a browser and completes on a localhost callback. On a remote host, forward
the callback port first:

```sh
ssh -L 54545:localhost:54545 user@host # anthropic
ssh -L 1455:localhost:1455 user@host # codex
```

Log in more than once per provider to pool several accounts; requests round-robin across
them. Credentials live under `~/.pengepul` (`0600`); a running relay picks up a fresh
login on restart or `pengepul accounts --reload`. Read the local API key clients use
from `pengepul config api-key`.

## Use it with openclaw

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
          { "id": "claude-opus-5", "api": "anthropic-messages", "contextWindow": 1000000, "maxTokens": 64000 }
        ]
      }
    }
  }
}
```

## Use it with hermes

Register pengepul as a named provider on the native Messages wire, written into
`HERMES_HOME/config.yaml`:

```sh
hermes config set model.provider pengepul
hermes config set model.default claude-opus-5
hermes config set providers.pengepul.base_url http://127.0.0.1:8317
hermes config set providers.pengepul.api_mode anthropic_messages
hermes config set providers.pengepul.api_key <pengepul api-key>
```

- `api_mode: anthropic_messages` forces the native wire on a root `base_url`, so the
  SDK appends `/v1/messages` and no extra route is needed.
- Use `provider: pengepul`, not `anthropic`. An `anthropic` provider makes hermes
  autodiscover the operator's `~/.claude` OAuth and route to `api.anthropic.com`,
  bypassing pengepul.
- Rotating `providers.*.api_key` inside an existing home caches the old key's rejection
  in `auth.json`; use a fresh home or delete `auth.json`.

## Build your own harness

pengepul is a plain REST relay, so any client that speaks the Anthropic or OpenAI API
can run on the pool. Point it at `http://127.0.0.1:8317` with the local API key, no
provider API key of your own.

Claude, on the Anthropic Messages API:

```sh
curl -sS http://127.0.0.1:8317/v1/messages \
  -H "Authorization: Bearer $API_KEY" -H "Content-Type: application/json" -d '{
  "model": "claude-opus-5",
  "max_tokens": 128,
  "messages": [{"role": "user", "content": "reply exactly: pong"}]}'
```

Codex, on the OpenAI Chat Completions API (a `gpt-5` or `codex-*` model routes to your
Codex subscription; `/v1/responses` works too):

```sh
curl -sS http://127.0.0.1:8317/v1/chat/completions \
  -H "Authorization: Bearer $API_KEY" -H "Content-Type: application/json" -d '{
  "model": "gpt-5.4",
  "messages": [{"role": "user", "content": "reply exactly: pong"}]}'
```

## Commands

| Command | Does |
|---|---|
| `pengepul serve` | start the relay (the default with no subcommand) |
| `pengepul login` | authorize an account in a browser |
| `pengepul status` | health of the running relay |
| `pengepul accounts` | loaded accounts (`--reload` re-reads from disk) |
| `pengepul update` | install the most recent release (`--check` only reports) |
| `pengepul config path\|show\|api-key` | show the config path, contents, or a key |
| `pengepul service install\|start\|stop\|restart\|status\|uninstall\|logs` | manage the user service |

Run `pengepul <command> --help` for flags.

## Reference

Routes: `POST /v1/messages`, `POST /v1/chat/completions`, `POST /v1/responses`,
`POST /v1/messages/count_tokens`, `GET /v1/models`, `GET /admin/accounts`,
`POST /admin/reload`, and `GET /health` (unauthenticated). Every route but `/health`
needs the local API key, as either `Authorization: Bearer <key>` or `x-api-key: <key>`.

The provider is chosen by model id: `gpt-5`, `gpt-5.*`, `gpt-5-*`, `o<N>` and `codex-*`
route to Codex, `claude-*` to Anthropic. `opus`, `sonnet` and `haiku` are aliases; a
request with no `model` is rejected with 400.

pengepul writes `~/.pengepul/config.yaml` when it is missing, generating a fresh
`sk-local-…` key. The keys you can set:

```yaml
host: '' # empty binds 127.0.0.1, not every interface
port: 8317
auth-dir: ~/.pengepul
api-keys:
  - sk-local-example
body-limit: 200mb # checked against Content-Length; empty means unlimited
timeouts:
  messages-ms: 120000
  stream-messages-ms: 600000
  count-tokens-ms: 30000
debug: off # off | errors | verbose
```

### Behavior

- Accounts are used strict round-robin across the pool, with no session affinity. A
  request fails over across accounts, retrying once per account on upstream 401, 403,
  429, 500 and 502-599, never on 501.
- A failed account backs off 1s, 2s, 4s, 8s, ... capped at 5 minutes, resetting on its
  next success. A dead refresh token locks it out for 24 hours, until a fresh
  `pengepul login`.
- Tokens refresh before expiry. A stream that ends without its completion sentinel
  counts as a failure, even after the client received a 200.

### Service

`pengepul service install` writes a systemd **user** unit on Linux or a launchd agent on
macOS:

```sh
pengepul service install --enable --start
pengepul service logs --follow
```

Because the unit is user-scoped, `systemctl status pengepul` will not find it. Use
`pengepul service status` and `pengepul service logs`, or add `--user`.
