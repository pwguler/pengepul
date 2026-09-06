# strip-thinking-suffix

## Goal

A model id ending in a client-side thinking level reaches the vendor with
that suffix removed, so `claude-opus-5:high` asks Anthropic for
`claude-opus-5` rather than for a model no vendor has ever heard of.

Reproducible today:

```
$ curl … -d '{"model":"anthropic/claude-opus-5:high", …}'
{"error":{"message":"model: claude-opus-5:high","type":"not_found_error"},
 "request_id":"req_011Cemm8tN4CX2GYWuiSpoVx"}
```

The `request_id` is Anthropic's. The relay strips the `anthropic/` prefix
and forwards the rest verbatim, suffix included.

## The rule

`:high` is pi's documented shorthand — *"Model with thinking level
shorthand"*, `pi --model sonnet:high`, README:656. It is client
vocabulary, and the relay already translates client vocabulary: it strips
provider prefixes, resolves aliases, and routes by name shape. Removing a
suffix the vendor cannot parse is the same job.

Only the seven levels pi defines are stripped — `off`, `minimal`, `low`,
`medium`, `high`, `xhigh`, `max` — matching pi's own
`splitKnownThinkingSuffix`. Anything else after a colon is part of the
name: the relay serves `commandcode/meituan/LongCat-2.0:free` today, and
ollama-style tags like `qwen:7b` are the same shape.

## Decisions

- **The relay strips it, not the client.** pi is not this repo, and the
  relay is already the layer that turns what a client says into what a
  vendor understands.
- **A known level, not everything after a colon.** One served model
  already ends in `:free`; a blanket rule would mangle it.
- **In `upstream_model`**, beside the prefix strip it already performs.
  That is the single seam deciding what the vendor is told, and usage
  counters key on the same value — so `:high` traffic and plain traffic
  stay one row per model rather than two.
- **Every provider, one rule.** Zero of 78 served models collide with a
  level today. pi applies the suffix regardless of provider, so the relay
  removes it the same way.
- **The level is discarded, not honoured.** Translating `:high` into an
  upstream thinking parameter would change behaviour for every client,
  not only pi. Out of scope, and named below.

## Non-goals

- **Not the subagent-judge failure.** That is a different defect at a
  different layer: `Model "pengepul/anthropic/claude-opus-5:high" not
  found. Use --list-models` carries no `request_id` and no HTTP request
  was sent — it fails inside pi's registry lookup, before the relay is
  reached. pi's `findModelInfo` already splits the suffix before looking
  up, so why that lookup fails is an open question in pi, not here. This
  spec does not unblock it.
- **Not honouring the level.** The relay will not turn `:high` into an
  upstream thinking parameter. Anthropic takes a `thinking` block and
  pengepul already sends one; wiring the suffix to it is a separate
  decision affecting every client.
- **Not advertising suffixed ids.** `/v1/models` keeps 78 entries. Seven
  levels per model would be 546, and the relay would assert levels it
  does not act on.
- **No change to routing.** A suffixed id already routes correctly — the
  404 proves the request reached Anthropic. Only what is sent onward
  changes.
- **No change to the plain or rich CLI output.**

## Acceptance criteria

- AC-1: `upstream_model` removes a trailing `:<level>` for each of the
  seven levels, after the provider prefix is stripped.
- AC-2: A trailing colon-word that is not one of the seven is kept.
  `LongCat-2.0:free` and `qwen:7b` reach upstream unchanged.
- AC-3: The strip applies for every provider kind — anthropic, codex and
  a configured generic endpoint alike.
- AC-4: Both `app.rs` call sites get the behaviour without their own
  copy of the rule; `grep` finds the level list in exactly one place.
- AC-5: A model id with no colon is unchanged, and a bare `:high` with
  nothing before it is left alone rather than becoming empty.
- AC-6: Only the final segment is considered: `a:high/b` is untouched,
  and `x:high:high` loses one level, not both.
- AC-7: The usage counters key on the stripped name, so a `:high`
  request and a plain request to the same model share one per-model row.
- AC-8: A request that today receives Anthropic's `not_found_error` for
  `claude-opus-5:high` receives a 200 after the change, exercised
  end to end against the running relay.
- AC-9: `/v1/models` still lists exactly the models it listed before.

## Verification

```bash
source ~/.cargo/env
cargo test --locked && cargo clippy --all-targets -- -D warnings && cargo fmt --check
TMPDIR=/home/kognos/tmp/a/rather/long/temp/prefix cargo test --locked

# AC-8, against the running relay:
KEY=$(grep -A 1 "^api-keys:" ~/.pengepul/config.yaml | tail -1 | sed 's/^- //')
curl -s -o /dev/null -w '%{http_code}\n' -X POST http://127.0.0.1:8317/v1/messages \
  -H "authorization: Bearer $KEY" -H 'content-type: application/json' \
  -d '{"model":"anthropic/claude-opus-5:high","max_tokens":8,
       "messages":[{"role":"user","content":"ok"}]}'   # expect 200, today 404

# AC-2, the model that must not break:
curl -s http://127.0.0.1:8317/v1/models -H "authorization: Bearer $KEY" \
  | grep -c 'LongCat-2.0:free'                          # expect 1
```

Each acceptance criterion needs a test that fails when its fix is
reverted.
