# pengepul

Run your own API relay for your AI subscriptions. Log in your Claude, ChatGPT/Codex and Grok
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
pengepul login --provider grok # authorize a grok.com account
pengepul serve # binds 127.0.0.1:8317
pengepul serve --host 0.0.0.0 --port 8317 # reachable across your network
```

Log in more than once per provider to pool accounts; requests rotate across them. A grok
account serves `grok-4.6` and `grok-4.5`, each with a 500k context window, named bare or
as `grok/grok-4.6`. Credentials live in `~/.pengepul` (`0600`); a running relay picks up a
fresh login on restart or `pengepul accounts --reload`. Read the key clients use with
`pengepul config api-key`. That key alone guards your subscriptions, so keep it secret and
prefer a trusted network or an SSH tunnel.

### OpenAI-compatible endpoints

Point the pool at any service speaking the OpenAI API. One command registers the
endpoint and saves its key; address its models as `<provider>/<model>`:

```sh
pengepul login --provider openrouter \
  --base-url https://openrouter.ai/api/v1 --key $OPENROUTER_API_KEY
systemctl --user restart pengepul   # or: pengepul service restart
```

A provider id becomes a directory under the auth dir: letters, digits, `.`, `-`, `_`;
never `.` or `..`, which a filesystem reads as somewhere else, and never a built-in's
name (`anthropic`, `claude`, `codex`, `grok`). Providers load at startup, so a new one
needs a restart. Registration only adds: an id already present with a different
`base-url` errors, naming the URL it kept, so a mistyped flag cannot move live traffic.
Repeating is safe; changing or removing one means editing the file.

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

`/v1/models` advertises what each endpoint's own `/models` publishes under these fields:
`context_window`, `context_length` or vLLM's `max_model_len`; `max_output_tokens`;
`input_modalities`; `reasoning`; and `pricing` as `*_per_million` rates. For models it
knows, pengepul fills, from its own table, the fields an endpoint leaves out. A local server
such as omlx publishes no `reasoning`, so a client like pi offers no thinking level for a
model that thinks. State it under the endpoint's `models:`, keyed by the id the endpoint
lists:

```sh
# ~/.pengepul/config.yaml
providers:
  omlx:
    base-url: http://10.10.1.25:8000/v1
    models:
      Qwen3.8-27B-Uncensored-MLX:
        reasoning: true
        context-window: 262144
        max-output-tokens: 32768
```

Each field is optional and wins over the endpoint's value and pengepul's table; a field
left out keeps what pengepul would advertise without it. A model the endpoint does not
list is not advertised, and an unknown field or a zero limit stops the relay at load.
`models:` is read at startup, so an edit needs a restart.

Login opens a browser and finishes on a localhost callback. On a remote host, forward the
port first:

```sh
ssh -L 54545:localhost:54545 user@host # anthropic
ssh -L 1455:localhost:1455 user@host # codex
ssh -L 14550:127.0.0.1:14550 user@host # grok
```

For grok the tunnel is optional: when the browser cannot reach its callback, auth.x.ai
shows a code instead, and pasting that code, or the full callback URL, at the login prompt
finishes the same login.

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
pengepul login --provider anthropic # authorize an account in a browser (--provider codex or grok for the others)
pengepul login --provider groq --key $KEY # save a static key for a configured provider
pengepul login --provider groq --base-url $URL --key $KEY # register a new OpenAI-compatible provider and save its key
pengepul status # health of the running relay: build, uptime, what each pool can serve
pengepul accounts # loaded accounts (-v per model, --reload re-reads from disk)
pengepul accounts disable|enable <id> # take an account out of its pool, or put it back and clear its cooldown
pengepul usage # the relay's numbers: 30-day trend, all-time peak, total, requests, tokens
pengepul update # install the most recent release (--check only reports)
pengepul config path|show|api-key # show the config path, contents, or a key
pengepul service install|start|stop|restart|status|uninstall|logs # manage the user service (systemd on Linux, launchd on macOS)
```

Run `pengepul <command> --help` for flags. The service is user-scoped, so
`systemctl status pengepul` will not find it: use `pengepul service status` or add
`--user`. On SIGTERM or SIGINT (a service stop, or Ctrl-C) the relay takes no new
connections and gives the requests in flight up to 15 s to finish, then exits.

## Reference

Routes: `POST /v1/messages`, `POST /v1/chat/completions`, `POST /v1/responses`,
`POST /v1/messages/count_tokens`, `GET /v1/models`, `GET /admin/accounts`,
`POST /admin/reload`, `POST /admin/accounts/disable`, `POST /admin/accounts/enable`, and
`GET /health` (unauthenticated). Every route but `/health` needs
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
    models: # optional, per model id the endpoint lists
      llama-3.3-70b-versatile:
        reasoning: false
        context-window: 131072
        max-output-tokens: 32768
body-limit: 200mb # largest request body the relay will read; empty means unlimited
timeouts:
  messages-ms: 120000 # deadline for a whole reply the client did not stream
  stream-messages-ms: 600000 # longest a streamed reply may send nothing
  count-tokens-ms: 30000
debug: off # off | errors | verbose
```

`stream-messages-ms` is a silence limit, not a deadline: a streamed reply runs as long as it
keeps sending, and fails once it sends nothing for that long, before it starts or between
chunks. codex streams every request upstream, so there it is also the deadline of a reply
the client did not stream.
