# 25. A model id reaches the vendor as the client wrote it

Status: Accepted (replaces `docs/specs/strip-thinking-suffix.md`, deleted)

## Context

`upstream_model` rewrote two things: a leading `<provider>/`, and a trailing
`:<thinking level>` from an allowlist of the seven words pi defines — `off`, `minimal`,
`low`, `medium`, `high`, `xhigh`, `max`. The second rewrite existed because a real client
sent one and the vendor refused it:

```
$ curl … -d '{"model":"anthropic/claude-opus-5:high", …}'
{"error":{"message":"model: claude-opus-5:high","type":"not_found_error"},
 "request_id":"req_011Cemm8tN4CX2GYWuiSpoVx"}
```

That `request_id` is Anthropic's: the suffix reached a vendor that has never heard of it.
`docs/specs/strip-thinking-suffix.md` fixed the symptom, and three of its own decisions are
why the fix does not survive review:

- **The level was discarded, not honoured.** The spec says so outright, in its own
  non-goals, and calls wiring the suffix to an upstream thinking parameter "a separate
  decision affecting every client". So the rewrite turned a failed request into a
  *successful* one, and the level the id named went nowhere: the request ran at whatever
  effort the body's own `reasoning_effort` asked for, that being the only channel held to the
  upstream. A client that wrote its level into the id and nowhere else asked for high and
  got the upstream's default, with a 200 saying otherwise.
- **The relay was maintaining a guess about a client's vocabulary.** The spec justifies its
  seven-word allowlist as "the same set pi's own `splitKnownThinkingSuffix` recognises".
  That function does not exist in the installed pi build (0.85.1), and no `:level` parser
  turned up in its dist — the client-side half of the contract is at least a version behind
  the name the relay cites, and pi's README documents the shorthand while the code that
  consumes it is not findable. A relay cannot maintain an allowlist for another project's
  CLI syntax it cannot see.
- **Shape cannot separate the two cases.** The allowlist was needed because
  `commandcode/meituan/LongCat-2.0:free` is a served model id and `qwen:7b` is an
  ollama-style tag: a colon-word is a vendor tag as often as it is a client shorthand. Only
  the seven words distinguished them, and only by the relay's assertion.

Also measured: the subagent-judge failure that the spec is often blamed for is a different
defect at a different layer. `Model "pengepul/anthropic/claude-opus-5:high" not found. Use
--list-models` carries no `request_id` and reached no relay — it fails inside pi's own
registry lookup.

## Decision

`upstream_model` strips the leading `<provider>/` and nothing else. A model id reaches the
vendor exactly as the client wrote it, a suffixed id included, and the usage counters key on
that same name.

The relay does not interpret client-side model-id vocabulary. A trailing colon-word is part
of the name, because the relay cannot tell a client's shorthand from a vendor's tag, and
guessing wrong is worse in both directions: it 404s a real model (the `:free` case) or
silently answers a request that asked for a different effort (the `:high` case).

Setting effort is the body's job and stays there: `reasoning_effort` is translated into the
upstream's thinking parameter (`translate.rs`, `thinking_from_effort`). The suffix was never
that channel, and a client that wants an effort must use the field.

## Consequences

- **A client that still appends a level now fails at the vendor**, with Anthropic's
  `not_found_error` naming the whole id. The failure is loud but the diagnosis is not: the
  vendor error does not say that the colon is the problem. That is the accepted cost of not
  rewriting a client's id, and it is the reason this ADR records the reproduction above.
- **The usage axis gains a row per id variant.** `LongCat-2.0:free` and a hypothetical
  `claude-opus-5:high` each key their own per-model row, which is the honest reading: those
  are the names the vendor was asked for. The panels treat a model name as an opaque string
  and already carry a colon in production.
- **The removed non-goal is moot rather than overturned.** "The level is discarded, not
  honoured" was a limitation of a rewrite that no longer exists. Nothing is discarded now,
  and the temptation it guarded against — quietly translating `:high` into a thinking
  budget — is the more likely mistake, since the id now travels verbatim and looks like it
  could be interpreted anywhere downstream.
- **Nobody re-adds the strip without reopening this.** The two tests that pin it are
  `upstream_model_strips_the_provider_prefix_and_nothing_else` and
  `a_model_id_reaches_the_vendor_whole_and_keys_its_own_usage_row`; both fail when a strip
  is reintroduced.

Rejected:

- **Keep stripping.** Cheapest for the client, but it answers a request for `:high` at
  medium effort and reports success, and it rests on a seven-word guess about another
  project's CLI.
- **Strip, and honour the level.** The right feature under the wrong mechanism: it changes
  behaviour for every client that ever typed a colon-word as part of a name, and it makes
  the relay's behaviour depend on a syntax it cannot verify.
- **Refuse a suffixed id with the relay's own 400.** Loud and self-explaining, but it
  invents a validation for a spelling the relay no longer claims to know anything about. The
  vendor already refuses it, and an id with a colon is a legitimate name.
