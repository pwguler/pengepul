# 16. Translate Messages onto a configured endpoint rather than refusing it

Status: Accepted (supersedes the inbound-dialect clause of ADR-0011)

## Context

ADR-0011 decided that a configured OpenAI-compatible endpoint "accept only Chat
Completions inbound — the common denominator every OpenAI-compatible client
speaks — with no new translators; Messages/Responses inbound and count_tokens
answer 501". That bought the whole generic-provider feature without touching the
translation layer, which was the right trade at the time.

It has since cost more than it bought. `route_request` sends only
`RequestRoute::Chat` to `ProviderKind::Generic`, so a Messages request for a
`<provider>/<model>` id is a 501 Refusal.

That refusal was invisible until a harness met it. Claude Code speaks Messages,
so every model behind a configured endpoint was unreachable from it: on a relay
with one anthropic pool and one configured endpoint, 11 of 78 advertised models
could be used and the other 67 answered 501 on the first prompt. The relay
advertised them on `/v1/models` all the same, because advertising follows the
catalog and the catalog does not know which dialect the client will arrive in.

The alternative considered was to keep the refusal and surface it earlier — mark
those models unavailable in `pengepul launch`'s picker and refuse them at
selection. That was built first. It answers the wrong question: it explains why
two thirds of the catalog is dead instead of making it live, and it puts a
per-dialect rule in the CLI that belongs to the relay.

## Decision

`translate.rs` and `streaming.rs` gain the missing dialect pair, and the Generic
arm of the route table accepts Messages:

- `anthropic_to_chat_request` — Messages request → Chat Completions request.
  A `system` field becomes a leading system message; a `tool_use` block becomes
  a `tool_calls` entry with stringified arguments; a `tool_result` block leaves
  its user turn to become a `tool` message of its own, after the turn that asked
  for it. Anthropic's server tools are dropped rather than mapped: a configured
  endpoint cannot run them, and a function that calls into nothing is worse than
  an absent one. Thinking blocks do not travel up — this dialect has nowhere to
  put them, and re-sending them as text would alter the transcript.
- `chat_to_anthropic_message` — the whole reply back.
- `chat_sse_to_anthropic` — the stream back. Chat Completions names no events
  and has none that opens a message, so the first chunk emits `message_start`
  and `finish_reason` closes it; tool calls are keyed by the `index` the chunks
  carry, because a call's id and name arrive on the chunk that opens it and its
  arguments trickle in afterwards.

Responses and `count_tokens` stay 501 for a configured endpoint. No client asks
for the first through that path, and the second is anthropic's own endpoint.

## Consequences

- ADR-0011's inbound clause no longer holds for Messages, and its stated reason
  — "no new translators" — is what this ADR spends. A configured Provider still
  speaks exactly one dialect *upstream*; what changed is that the relay closes
  the inbound gap instead of declining to. The rest of ADR-0011 stands: prefix-only
  routing, degenerate tokens, no cloaking, failover that never crosses endpoints.
- Every advertised model is usable from every harness pengepul launches, so the
  notion of a model a harness cannot be given is gone — with it went the picker's
  `unavailable` marking and the `--model` dialect guard.
- Each translation is a place fidelity can be lost, and two are known and
  deliberate: server tools are dropped, and thinking blocks do not survive a
  round trip back upstream. A model that depends on either behaves differently
  through the relay than against its own API.
- Usage counters come from the endpoint's own `prompt_tokens`/`completion_tokens`.
  A gateway that reports usage only in a final chunk after `finish_reason` leaves
  `output_tokens` at zero for that stream; the counter is honest about what was
  observed rather than estimating.
- Responses remains the one refused dialect there, so the Refusal path and its
  accounting stay exercised.
