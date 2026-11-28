# 35. A request with no limit gets the model's output ceiling

Status: Accepted

## Context

The Messages dialect requires `max_tokens`; Chat Completions and Responses do not. When a
request in either of those reached the anthropic **Upstream**, `openai_to_anthropic` filled
the missing field with `8192`. That number was the relay's own: no client asked for it and no
model is limited to it. An opus reply with thinking spends part of it reasoning and stopped
at exactly 8192 output tokens. Hermes sends its context-compression summary on Chat with no
limit on purpose, and the live relay logged `output=8192` on every attempt while Hermes
logged `summary was truncated (finish_reason=length)` seventeen times: compression of a
large session never completed.

## Decision

When a translated request names no output limit, the relay asks the anthropic upstream for
the served model's **Output ceiling**: the `max_output_tokens` of the catalog entry the last
fetch listed, else of the curated table, else 64000. A limit the client set reaches the
upstream as written. A native Messages request is not touched.

The ceiling is read where the request is routed, from the catalog the relay already holds,
so the translator no longer invents a value: it copies a limit or leaves the field out.

64000 is the smallest ceiling of any current anthropic family in the curated table (opus 4,
sonnet 4, haiku 4.5), so every current model accepts it, and it is above the largest
thinking budget the relay derives from `reasoning_effort` (24576), so the Messages rule
`max_tokens > budget_tokens` holds for an invented limit. A model too old for 64000 answers
with the vendor's own 400 naming its maximum, which is a clearer failure than a silent cut.

## Considered options

- **A larger fixed default** (128000). It lost because models in the table cap at 64000 and
  would refuse it, and a number the relay picks for every model is the defect itself at a
  bigger size.
- **Pass the ceiling into the translator.** It lost because the translator is a pure
  rewrite of one body, and only the anthropic route both needs a limit and holds the catalog.
- **Raise a client limit past the thinking budget**, as the antigravity translation does for
  Gemini. It lost because the client's request reaches the vendor as written (ADR-0025);
  a client that sets a limit below its own effort's budget gets the vendor's 400.

## Consequences

- A request with no limit can run for as long as the model writes. That is what the client
  asked for by naming none.
- A new curated or fetched `max_output_tokens` changes what such a request is asked for, with
  no code change.
