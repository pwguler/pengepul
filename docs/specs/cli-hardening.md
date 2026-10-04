# cli-hardening

## Goal

The standalone relay stops cutting long streams, takes an upstream's retry hint as the length
of a Cooldown, drains requests on shutdown, refuses `grok` as a configured provider id, gives
every model its own metadata, and its docs describe the code as it is.

## Non-goals

- **No admin layer.** `/admin/*` keeps accepting the Local API keys in `config.yaml`, exactly
  as today. Only pengepul-ai has an admin layer.
- **No change to Failover, to the Cooldown ladder's base and ceilings, or to ADR-0027.** A
  retry hint only lengthens a Cooldown that a Pool of two or more already earns.
- **No retry hint on responses to clients.** The upstream's response reaches the client as it
  does today.
- **A request the client did not stream keeps a whole-request deadline,** with the value it
  has today, and `X-Stainless-Timeout` keeps reading `stream-messages-ms`.
- **No new config key, no new CLI verb, and no change to the service unit files.**
- **Code that only drifted from its docs is not changed to match them.**
  `transform_sse_event` keeps its own match over the Provider and the Inbound dialect.
- **Nothing for pengepul-ai.**

## Decisions

- **Stream deadline:** `stream-messages-ms` is a silence limit, the longest an upstream stream
  may send nothing, before its response starts and between chunks. A deadline on the whole
  response cut healthy long generations and put the Account that served them on Cooldown.
- **Retry hint:** a 429 or 503 carrying `retry-after-ms`, or `retry-after` as seconds or an
  HTTP date, cools the Account for the longer of the escalating backoff and the hint, capped
  at 24 h. A vendor that names its reset beats probing at 1 s, 2 s, 4 s, and the cap bounds a
  nonsense value.
- **Shutdown:** SIGTERM and SIGINT stop new connections and give requests in flight 15 s. That
  fits inside launchd's default of about 20 s and systemd's 90 s before either kills the
  process, so the unit files stay as they are.
- **`grok` is a built-in name:** a configured `grok` entry stops the relay at load, like
  `anthropic`, `codex` and `claude`. Accepted, it was shadowed by the built-in Pool's prefix
  and shared that Pool's `usage.json`.
- **Docs follow the code,** never the reverse.
- **No admin in the CLI:** an admin credential belongs to pengepul-ai.

## Acceptance criteria

- AC-1: A streamed request whose upstream keeps sending runs to completion past
  `stream-messages-ms` of total time, and its Account records a success.
- AC-2: A streamed request whose upstream sends nothing for longer than `stream-messages-ms`,
  before its response starts or between chunks, ends with an error the client receives, and
  its Account records a failure whose `lastError` names the silence.
- AC-3: In a Pool of two or more, a 429 or 503 carrying `retry-after: 3600` puts the Account
  on a 3600 s Cooldown, on streamed and non-streamed requests alike. `retry-after-ms` and an
  HTTP-date `retry-after` are read the same way, and a hint above 24 h is capped at 24 h.
- AC-4: A hint never shortens a Cooldown. One shorter than the escalating backoff, or one that
  arrives during a Reauth Cooldown, leaves the longer Cooldown in place; an absent or
  unparseable hint leaves today's Cooldown.
- AC-5: A Pool of one earns no Cooldown from a 429 that carries a hint (ADR-0027).
- AC-6: On SIGTERM or SIGINT the relay stops accepting connections, lets a request already in
  flight finish, and exits.
- AC-7: A request still running 15 s after the signal does not keep the process alive past the
  15 s mark.
- AC-8: A `config.yaml` naming `grok` under `providers:` stops the relay at load with the
  built-in-name error.
- AC-9: A `/models` body holding an entry without its `id` (OpenAI-compatible) or `slug`
  (codex) leaves every other model with its own metadata.
- AC-10: README, ARCHITECTURE and CONTEXT describe the code as it is: the grok Provider (its
  login, the 14550 callback and the pasted-code fallback, its models), `pengepul launch`, what
  `stream-messages-ms` means, the retry hint in every Cooldown description, the 15 s drain,
  Grok in `ProviderKind`, and stream events translated outside `Translation`. The three stale
  comments are corrected: `can_ask`'s, the duplicate in `transform_sse_event`, and the
  `cap_cache_control` doc that sits on `CHECKPOINT_STRIDE`. Judge: `rubric`, against the code.

## Verification

```
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
rubric: AC-10, README, ARCHITECTURE and CONTEXT read against the code
```

Seen working against real accounts: with a 5 s silence limit, a reply that streams for far
longer than 5 s arrives whole, and a request in flight when SIGTERM lands still finishes.

```
sed 's/stream-messages-ms: .*/stream-messages-ms: 5000/' ~/.pengepul/config.yaml > /tmp/silence.yaml
pengepul serve --config /tmp/silence.yaml --port 8318 & RELAY=$!; sleep 2
KEY=$(pengepul config api-key)
ask() { curl -sN localhost:8318/v1/messages -H "Authorization: Bearer $KEY" \
  -H 'Content-Type: application/json' \
  -d "{\"model\":\"claude-opus-5\",\"max_tokens\":8000,\"stream\":true,\"messages\":[{\"role\":\"user\",\"content\":\"$1\"}]}"; }
ask 'Write 3000 words about rivers.' | tail -n 2           # ends in message_stop
ask 'Reply with one word.' > /tmp/short.sse & sleep 0.5
kill -TERM $RELAY; wait $RELAY; tail -n 2 /tmp/short.sse   # reply finished, relay gone within 15 s
```
