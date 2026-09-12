# 23. A cache read is not input

Status: Accepted

## Context

The OpenAI dialects report the cache counts **inside** the input count. OpenAI's own guide
shows `input_tokens: 15000` beside `input_tokens_details: {cached_tokens: 12000,
cache_write_tokens: 3000}`, and that partition is explicit in its generated types:
`cached_tokens` is *"cached tokens present in the prompt"*. Codex's own client subtracts it —
`non_cached_input() = (input_tokens - cached_input()).max(0)` in
`codex-rs/protocol/src/protocol.rs`. Groq states it as arithmetic: *"Cache Hit Rate =
cached_tokens / prompt_tokens × 100%"*. xAI, OpenRouter and Gemini follow the same
convention, and Moonshot/Kimi reports a flat `usage.cached_tokens` that is also a slice.

Anthropic is the other way round, and DeepSeek too. Anthropic's docs: *"`input_tokens`:
Number of input tokens which were not read from or used to create a cache (that is, tokens
after the last cache breakpoint)"*, with `total_input_tokens = cache_read_input_tokens +
cache_creation_input_tokens + input_tokens`. DeepSeek publishes no `cached_tokens` at all,
only `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`.

The relay recorded `input_tokens` raw, so `carried_tokens = input + output + cache_read +
cache_write` (`src/usage_view.rs`) counted the cache read twice for every Provider except
Anthropic and DeepSeek. Measured on a live pool: `commandcode` held `input` 514,634,436
beside `read` 498,866,688 — under the OpenAI convention its uncached input was ~15.7M, and
the two counters were within 3% of each other because one contained the other. Every figure
derived from those counters inherited it, including ADR-0017's "cache share of input tokens"
table, whose denominator contains its numerator on those Providers.

Gaps came with it: `cache_write_tokens` was never read, so the Codex and OpenRouter cache
writes the vendors bill at 1.25× were structurally zero; Moonshot/Kimi's flat
`cached_tokens` and DeepSeek's `prompt_cache_hit_tokens` were unread, so those pools reported
zero cache reads; and xAI's chat dialect reports reasoning *outside* `completion_tokens`
while counting it inside `total_tokens` (its own example: `32 + 9 + 110 = 151`), so excluding
reasoning from carried load dropped it.

## Decision

`input_tokens` means the **uncached** input on every Provider, and `output_tokens` means
**every** token generated including reasoning. The four counters are disjoint everywhere, so
`carried_tokens` needs no knowledge of dialects and each token the upstream billed is
counted once.

Each vendor's field names are read where they live, and the normalization happens in
`separate_cache_from_input` at `record_provider_success` — the one seam every Provider passes
through exactly once per request. The dialect comes from `provider.kind`, which describes the
upstream that produced the body rather than the shape of the body: a Messages request served
by a configured OpenAI-compatible endpoint is answered with an Anthropic-shaped body for the
client, but the recorded usage was read from the endpoint's own chat body.

The reasoning fold uses the vendor's own arithmetic as the discriminant: reasoning is folded
into output when a body reports `total_tokens == input + output + reasoning` with reasoning
non-zero. That gets both of xAI's wires right — its Responses dialect reports reasoning inside
`output_tokens` (`131 + 624 = 755`) and is left alone — without a table of dialects.

Rejected: keeping `input_tokens` raw per dialect and teaching every view the dialect. It
would need the dialect persisted beside each counter in `usage.json`, changing the file format
for a property the writer already knows. Rejected: `saturating_sub` as the floor for the
subtraction. It saturates at `i64::MIN`, not at zero; `more_cached_tokens_than_input_records_
no_negative_input` records −4990 without the explicit `.max(0)`.

## Consequences

- **Counters recorded before this change stay as they are.** `usage.json` does not store the
  dialect, so an OpenAI-dialect pool's existing `input` total cannot be told apart from an
  uncached one and cannot be re-derived. New outcomes are correct from the first request after
  the upgrade; the panel's `in` column drops for those pools at that point, and nothing
  backfills it. Same rule as counters recorded before per-model attribution existed.
- **The client-visible usage on a translated route is unchanged** and still carries the raw
  vendor numbers. A harness that computes its own hit rate through the relay is still wrong on
  those routes; that is a separate change, and
  `docs/research/cache-usage-fields-by-provider.md` says which fields each side would need.
- **The field list is now the thing that drifts.** A vendor added to that research file
  belongs in `cache_read_from` / `cache_write_from` too, or its cache reads are recorded as
  zero — the failure mode this ADR exists to stop, one Provider at a time.
- xAI's chat reasoning now lands in `output_tokens`, so `carried load` for the grok pool is
  the `total_tokens` xAI billed. The `reasoning` column remains a breakdown of it.
- Anthropic's `usage.iterations[]` is deliberately not summed, and
  `an_anthropic_iterations_array_is_not_summed_into_the_counters` now fails if a later reader
  starts walking it.
