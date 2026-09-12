# cache-counter-semantics

## Goal

Record `input_tokens` as the uncached input on every Provider, so the OpenAI dialects stop counting a cache read twice and the Grok pool stops reporting zero cache reads.

## Non-goals

- The usage the relay hands back to a client on a translated route. `update_chat_usage` and `update_responses_usage` in `src/streaming.rs`, and the usage converters in `src/translate.rs`, keep today's behaviour.
- Migrating `usage.json`. A file written before this change carries input tokens that include the cache read for OpenAI dialects, and the file does not store the dialect, so the two cannot be told apart. Counters recorded before this change stay as they are, exactly as counters recorded before per-model attribution did.
- Cost. The relay prices nothing; `pricing` in `src/models.rs` stays catalog metadata.
- Rotation, affinity and `cache_control` rewriting. Unchanged.
- A 1h share for the OpenAI dialects. They publish no equivalent of `cache_creation.ephemeral_1h_input_tokens`, so `cache_creation_1h_input_tokens` stays 0 there.

The field names and the inclusion semantics below are cited in
`docs/research/cache-usage-fields-by-provider.md`; that file, not this one, owns the
question of what each vendor reports.

## Acceptance criteria

- AC-1: `input_tokens` means uncached input for every ProviderKind. Anthropic and the OpenAI dialects both report `input + cache_read + cache_creation` as the total prompt.
- AC-2: An OpenAI-dialect usage carrying `prompt_tokens: 21` with `cached_tokens: 4` records `totalInputTokens` 17 and `totalCacheReadInputTokens` 4. `chat_completions_route_records_generic_stream_usage` asserts 21 today; it asserts 17 when this lands.
- AC-3: An Anthropic usage carrying `input_tokens: 100`, `cache_read_input_tokens: 2`, `cache_creation_input_tokens: 1` still records `totalInputTokens` 100. The subtraction is keyed on the dialect, not on the field name.
- AC-4: A **flat** `usage.cached_tokens` is read, beside the nested `prompt_tokens_details.cached_tokens`. Moonshot/Kimi is the vendor that reports it that way; the nested-only reader records 0 for it today.
- AC-5: A **disjoint** cache report is read. DeepSeek publishes `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens` and no `cached_tokens` at all. Its hit is read as the cache read, and subtracting that hit from `prompt_tokens` yields exactly the miss count DeepSeek reports separately, so the two fields agree by construction rather than by a second rule.
- AC-6: `cache_write_tokens` is recorded as `totalCacheCreationInputTokens` for every dialect that publishes it — `input_tokens_details.cache_write_tokens` (Responses, Codex) and `prompt_tokens_details.cache_write_tokens` (Chat, OpenRouter) — and subtracted from the input beside the cache read.
- AC-7: The subtraction saturates at 0. An upstream that reports more cached tokens than prompt tokens cannot record a negative input count.
- AC-8: `carried_tokens` is the same number for the same true prompt whether the upstream was Anthropic or OpenAI-dialect.
- AC-9: An Anthropic usage that also carries `usage.iterations[]`, each entry with its own input and cache counters, records the top-level counters once. Today nothing pins that, so a future change that sums every counter it finds would silently multiply them.
- AC-10: `output_tokens` means every token the model generated, reasoning included. The Anthropic and OpenAI dialects already report it that way — the Anthropic SDK states *"`output_tokens` remains the inclusive, authoritative total used for billing"* — and reasoning is folded into it where the vendor keeps the two apart, which is xAI's Chat dialect (`prompt_tokens: 32, completion_tokens: 9, reasoning_tokens: 110, total_tokens: 151`). After this, reasoning is a breakdown of `output_tokens` on every Provider, which is the assumption `carried_tokens` already makes.

  The rule is total, so it also covers Gemini's `thoughtsTokenCount` beside
  `candidatesTokenCount` — but **no test covers Gemini and none can from here**: the relay
  never speaks the native API, `thoughtsTokenCount` appears in no code, and Google's
  OpenAI-compatible usage shape is itself unverified in the research file. The Gemini half of
  this criterion is a vendor fact recorded for the next dialect, not a tested path.

## Verification

One line per criterion. Eight of these fail against the pre-change source — a judge
verified that by running the new tests with `HEAD`'s `src/app.rs` — and two are guards that
pass before the change and fail against a *wrong* one: AC-3's fails if an Anthropic input is
reduced twice, AC-9's if a later reader starts summing `usage.iterations[]`. They are
evidence about the shape of the change, not about its absence.

```
cargo test --test app chat_completions_route_records_generic_stream_usage   # AC-1, AC-2 (chat)
cargo test --test app a_codex_usage_records_only_the_uncached_input        # AC-1, AC-2 (responses)
cargo test --test app an_anthropic_usage_keeps_its_input_beside_its_cache_counts  # AC-3, AC-8
cargo test --test app a_flat_cached_tokens_is_read_beside_the_nested_one   # AC-4
cargo test --test app a_deepseek_cache_hit_becomes_the_cache_read_and_the_miss_the_input  # AC-5
cargo test --test app a_codex_cache_write_is_recorded_as_a_cache_write     # AC-6, AC-8
cargo test --test app a_chat_cache_write_is_recorded_as_a_cache_write      # AC-6
cargo test --test app more_cached_tokens_than_input_records_no_negative_input  # AC-7
cargo test --test app an_anthropic_iterations_array_is_not_summed_into_the_counters  # AC-9
cargo test --test app output_tokens_include_reasoning_where_the_vendor_counts_it_outside  # AC-10
cargo test --lib usage_from_response
cargo test --test accounts
cargo test --locked --all-targets --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo fmt --check
```
