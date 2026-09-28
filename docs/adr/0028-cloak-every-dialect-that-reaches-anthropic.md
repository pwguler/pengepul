# 28. Cloak every dialect that reaches anthropic

Status: Accepted (amends ADR-0002, ADR-0004 and ADR-0007)

## Context

ADR-0002 and ADR-0004 ran the Cloaking sanitizer from `route_anthropic_request` on
the Messages route only, and ADR-0007 kept it there by having each client configure
the Messages wire. Nothing enforces that config. hermes set to
`api_mode: chat_completions` posts to `/v1/chat/completions`, and its request
reached anthropic translated into Messages. The billing-header block and the Claude
Code prefix were injected, but its own system prompt and `snake_case` tool names went
through untouched, and the Classifier answered 400. The Classifier reads the body. It
never sees the dialect the client spoke.

## Decision

The sanitizer runs on every request `route_anthropic_request` sends, after the
translation into Messages, so a Chat- or Responses-shaped request is Cloaked like a
native one. Tool names are restored on the upstream's Messages-shaped reply, before
translation moves each name into the client's dialect, where the restore cannot read
it (`tool_calls[].function.name` in Chat).

## Considered options

- **Keep the sanitizer on Messages and rely on client config (ADR-0007).** Rejected:
  a harness on another wire, by default or by mistake, gets a hard 400, and nothing
  in the relay points at the cause (Nothing observes the Classifier).

## Consequences

- ADR-0002's "every `POST /v1/messages` and nowhere else" does not hold: the rename
  runs on every dialect that reaches anthropic, and every reply path, JSON and each
  streamed dialect, restores the names before translation.
- ADR-0007's route rule stands: no per-client routes. Its note that the sanitizer
  stays a Messages-route transform does not.
- ADR-0004's Codex clause stands: a Codex-backed model never reaches the sanitizer.
- ADR-0008's web-tool swap reaches Chat and Responses clients too. A function tool
  named `web_search` or `web_fetch` runs upstream as the server tool, and the Chat
  translation keeps its blocks out of `tool_calls`, streamed input included, so the
  client never gets a call it would try to dispatch.
- `count_tokens` keeps identifying headers only, with no body Cloaking.
