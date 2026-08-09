# pengepul

Keep your Claude subscription working inside openclaw and hermes. Point the agent at
pengepul instead of the Anthropic API, and your Max/Pro plan answers the request — no
per-token API bill.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/gitshrl/pengepul/main/install.sh | sh
```

Linux x86_64 and macOS on Apple silicon. `pengepul update` installs the most recent
release (`--check` reports it without installing); both verify the published checksum.
From source: `cargo install --git https://github.com/gitshrl/pengepul.git --locked`.

## Quickstart

```sh
pengepul login    # opens a browser to authorize your Anthropic account
pengepul serve    # listens on 127.0.0.1:8317
```

`login` completes on a localhost callback; on a remote host, forward the port first
with `ssh -L 54545:localhost:54545 user@host`. Credentials live under `~/.pengepul`
(`0600`); a running relay picks up a fresh login on restart or `pengepul accounts
--reload`. Read the key clients authenticate with from `pengepul config api-key`.

## Use it with openclaw

The embedded runner talks native Anthropic Messages. In `~/.openclaw/openclaw.json`,
register a `pengepul` provider and select it with a `pengepul/`-prefixed model — a bare
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
- Use `provider: pengepul`, not `anthropic` — an `anthropic` provider makes hermes
  autodiscover the operator's `~/.claude` OAuth and route to `api.anthropic.com`,
  bypassing pengepul.
- Rotating `providers.*.api_key` inside an existing home caches the old key's rejection
  in `auth.json`; use a fresh home or delete `auth.json`.

## Reference

Point a client at `http://127.0.0.1:8317` with the local API key, as either header:

```
Authorization: Bearer <key>
x-api-key: <key>
```

Routes: `POST /v1/messages`, `POST /v1/messages/count_tokens`, `GET /v1/models`,
`GET /admin/accounts`, `POST /admin/reload`, and `GET /health` (unauthenticated).
`opus`, `sonnet` and `haiku` are model aliases; a missing `model` becomes
`claude-sonnet-4-6`.

pengepul writes `~/.pengepul/config.yaml` when it is missing, generating a fresh
`sk-local-…` key. The keys you can set:

```yaml
host: ''                    # empty binds 127.0.0.1, not every interface
port: 8317
auth-dir: ~/.pengepul
api-keys:
  - sk-local-example
body-limit: 200mb           # checked against Content-Length; empty means unlimited
timeouts:
  messages-ms: 120000
  stream-messages-ms: 600000
  count-tokens-ms: 30000
debug: off                  # off | errors | verbose
```

Unknown keys are a hard load error. `RUST_LOG` overrides the log level. Commands:
`serve` · `login` · `status` · `accounts` · `update` · `config path|show|api-key` ·
`service …`. Run `pengepul <cmd> --help` for flags.

### Behavior

- Accounts are used strict round-robin, with no session affinity. A request fails over
  across accounts, retrying once per account on upstream 401, 403, 429, 500 and
  502-599, never on 501.
- A failed account backs off 1s, 2s, 4s, 8s, … capped at 5 minutes, resetting on its
  next success. A dead refresh token locks it out for 24 hours, until a fresh
  `pengepul login`.
- Tokens refresh once expiry is under 4 hours away. A stream that ends without its
  completion sentinel counts as a failure, even after the client received a 200.

### Service

`pengepul service install` writes a systemd **user** unit on Linux or a launchd agent
on macOS:

```sh
pengepul service install --enable --start
pengepul service logs --follow
```

Because the unit is user-scoped, `systemctl status pengepul` will not find it — use
`pengepul service status` and `pengepul service logs`, or add `--user`.
