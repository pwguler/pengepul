# 25. A model id reaches the vendor as the client wrote it

Status: Accepted (replaces `docs/specs/strip-thinking-suffix.md`, deleted)

## Context

`upstream_model` rewrote two things: a leading `<provider>/`, and a trailing
`:<thinking level>` from an allowlist of the seven words pi defines — `off`, `minimal`,
`low`, `medium`, `high`, `xhigh`, `max`. The second rewrite existed because a suffixed id
names no model any vendor has, and the vendor says so:

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
- **The relay was mirroring a rule it does not own, and the client already applies it.** The
  spec justifies its seven-word allowlist as "the same set pi's own
  `splitKnownThinkingSuffix` recognises". The rule is real, and it lives in the client twice:
  the harness core holds the list as `VALID_THINKING_LEVELS` and its resolver splits
  `id:level` into a model plus a thinking level before any request is built
  (`@earendil-works/pi-coding-agent/dist/cli/args.js`, `dist/core/model-resolver.js`), and
  the `pi-subagents` extension splits the same names for its own scope and display purposes
  and then *re-attaches* the suffix to the model string it hands the core
  (`pi-subagents/src/shared/model-info.ts`, `src/runs/shared/child-tool-plan.ts`). The
  relay's copy was a third one, redundant for the client it was written for, and owned by a
  component that cannot see either of the other two change.
- **Shape cannot separate the two cases.** The allowlist was needed because
  `commandcode/meituan/LongCat-2.0:free` is a served model id and `qwen:7b` is an
  ollama-style tag: a colon-word is a vendor tag as often as it is a client shorthand. Only
  the seven words distinguished them, and only by the relay's assertion.

Also measured: the subagent-judge failure that the spec is often blamed for is a different
defect at a different layer. `Model "pengepul/anthropic/claude-opus-5:high" not found. Use
--list-models` carries no `request_id` and reached no relay — it fails inside the harness
core's registry lookup, before any request is built.

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

`/v1/models` keeps advertising one id per model, never `<id>:<level>`. The relay does not act
on a level, so advertising seven variants of each model would assert behaviour it does not
have — the non-goal the deleted spec also recorded.

## Consequences

- **A client that still appends a level fails at the vendor, and the answering account pays
  for it.** The vendor answers `not_found_error` naming the whole id: loud, but the error
  never says the colon is the problem. The refusal is then booked as account health. A 404 is
  not one of the statuses the relay routes to its Refusal path, so it is classified `network`
  and earns the account that served it a failure streak and a cooldown; the next request that
  carries the same id falls through rotation to another account and does it again, until
  every account is cooled and clients get `503 no available … last failure: network` instead
  of the vendor's error. No per-model row is opened, because those are written on success.
  None of this is new handling — any unknown model id has always been booked this way — but
  a spelling that used to succeed now takes this path.
- **What was measured, and what it does not cover.** Client-side records do carry suffixed
  ids: 19 across the retained pi sessions, every one of them inside `subagent`/`completions`
  arguments or results, none as a session's own model — and the client resolves those before
  dispatch (above). The relay's own per-model rows, keyed on the name the vendor was asked
  for, hold no level-suffixed row between 2026-09-05 and 2026-09-14. Neither set observes the
  wire from claude, openclaw or hermes: those were checked by configuration only, so a
  suffixed id from one of them would surface as the failure described above.
- **The usage axis gains a row per id variant the vendor serves.** An id the vendor accepts
  keys its own per-model row, which is the honest reading: that is the name the vendor was
  asked for. The panels treat a model name as an opaque string and already carry a colon in
  production (`meituan/LongCat-2.0:free`). An id the vendor refuses opens no row at all.
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

- **Keep stripping.** Cheapest for the client, but it answers a request whose id named a
  level, reports success, and lets that level go nowhere unless the body also carried it —
  and it rests on a seven-word guess about another project's syntax.
- **Strip, and honour the level.** The right feature under the wrong mechanism: it changes
  behaviour for every client that ever typed a colon-word as part of a name, and it makes
  the relay's behaviour depend on a syntax it cannot verify.
- **Refuse a suffixed id with the relay's own 400.** This is the gentler option and it was
  rejected on incomplete grounds once, so the comparison is recorded here in full: a relay
  400 takes the Refusal path, which counts the request and touches no account health, and the
  client gets a message naming the suffix instead of the vendor's unhelpful
  `not_found_error`. It is rejected anyway because it means validating a spelling the relay
  no longer claims to know anything about, and every future change to the rule would then
  land in two places. Anyone reopening this should weigh that against the account cooldown
  the chosen path charges to a client's id.
