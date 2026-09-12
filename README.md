# pengepul

Run your own API relay for your AI subscriptions. Log in your Claude and ChatGPT/Codex
accounts once and every request is served from the pool, so your harness runs on your
subscription instead of a per-token key.

- Pools several subscription accounts per provider and spreads requests across them.
- Serves your subscription inside openclaw and hermes, with no API key.
- Relays any OpenAI-compatible API (groq, openrouter, deepseek, ...) through the pool.
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
pengepul login --provider anthropic # authorize an Anthropic account
pengepul login --provider codex # authorize a ChatGPT/Codex account
pengepul serve # binds 127.0.0.1:8317
pengepul serve --host 0.0.0.0 --port 8317 # reachable across your network
```

Log in more than once per provider to pool accounts; requests rotate across them.
Credentials live in `~/.pengepul` (`0600`); a running relay picks up a fresh login on
restart or `pengepul accounts --reload`. Read the key clients use with `pengepul config
api-key`. That key alone guards your subscriptions, so keep it secret and prefer a trusted
network or an SSH tunnel.

### OpenAI-compatible endpoints

Point the pool at any service speaking the OpenAI API. One command registers the
endpoint and saves its key; address its models as `<provider>/<model>`:

```sh
pengepul login --provider openrouter \
  --base-url https://openrouter.ai/api/v1 --key $OPENROUTER_API_KEY
systemctl --user restart pengepul   # or: pengepul service restart
```

A provider id becomes a directory under the auth dir: letters, digits, `.`, `-`, `_`;
never `.` or `..`, which a filesystem reads as somewhere else. Providers load at startup, so a new one needs a restart. Registration
only adds: an id already present with a different `base-url` errors, naming the URL it
kept, so a mistyped flag cannot move live traffic. Repeating is safe; changing or removing
one means editing the file.

Registration rewrites `config.yaml`: values survive, comments do not. It holds a
`config.yaml.lock` for the write, and a killed registration can leave that lock behind.
The next one then names the file and stops, and removing it is the whole recovery.

Editing the file by hand works too: one entry per endpoint, then `pengepul login
--provider <id> --key $KEY`:

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

Configured endpoints speak Chat Completions and rotate across their keys with the same
failure handling as subscription providers.

Login opens a browser and finishes on a localhost callback. On a remote host, forward the
port first:

```sh
ssh -L 54545:localhost:54545 user@host # anthropic
ssh -L 1455:localhost:1455 user@host # codex
```

## Clients

### openclaw

The embedded runner speaks native Anthropic Messages. In `~/.openclaw/openclaw.json`,
register a `pengepul` provider and select it with a `pengepul/`-prefixed model; a bare
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

Register pengepul on the native Messages wire in `HERMES_HOME/config.yaml`:

```sh
hermes config set model.provider pengepul
hermes config set model.default claude-opus-5
hermes config set providers.pengepul.base_url http://127.0.0.1:8317
hermes config set providers.pengepul.api_mode anthropic_messages
hermes config set providers.pengepul.api_key <pengepul api-key>
```

- `api_mode: anthropic_messages` forces the native wire. `base_url` may be the root or
  end in `/v1`.
- Use `provider: pengepul`, not `anthropic`: that makes hermes autodiscover `~/.claude`
  OAuth and route to `api.anthropic.com`, bypassing pengepul.
- Rotating `providers.*.api_key` caches the old key's rejection in `auth.json`; use a
  fresh home or delete it.

### Your own harness

pengepul is a plain REST relay, so any client speaking the Anthropic or OpenAI API can
run on the pool. Point it at `http://127.0.0.1:8317/v1` with the local API key; a root
URL without `/v1` works too.

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
pengepul login --provider anthropic # authorize an account in a browser (--provider codex for Codex)
pengepul login --provider groq --key $KEY # save a static key for a configured provider
pengepul login --provider groq --base-url $URL --key $KEY # register a new OpenAI-compatible provider and save its key
pengepul status # health of the running relay
pengepul accounts # loaded accounts (--reload re-reads from disk)
pengepul usage # the last 30 days of tokens, as a sparkline
pengepul update # install the most recent release (--check only reports)
pengepul config path|show|api-key # show the config path, contents, or a key
pengepul service install|start|stop|restart|status|uninstall|logs # manage the user service (systemd on Linux, launchd on macOS)
```

Run `pengepul <command> --help` for flags. The service is user-scoped, so
`systemctl status pengepul` will not find it: use `pengepul service status` or add
`--user`.

## Reference

Routes: `POST /v1/messages`, `POST /v1/chat/completions`, `POST /v1/responses`,
`POST /v1/messages/count_tokens`, `GET /v1/models`, `GET /admin/accounts`,
`POST /admin/reload`, and `GET /health` (unauthenticated). Every route but `/health` needs
the local API key, sent as `Authorization: Bearer <key>` or `x-api-key: <key>`.

pengepul writes `~/.pengepul/config.yaml` when missing, with a fresh `sk-local-…` key. The
keys you can set:

```yaml
host: '' # empty binds 127.0.0.1, not every interface
port: 8317
auth-dir: ~/.pengepul
api-keys:
  - sk-local-example
providers:
  groq:
    base-url: https://api.groq.com/openai/v1
body-limit: 200mb # largest request body the relay will read; empty means unlimited
timeouts:
  messages-ms: 120000
  stream-messages-ms: 600000
  count-tokens-ms: 30000
debug: off # off | errors | verbose
```
