# How each provider reports cache token counts

Researched 2026-09-12 against vendors' own API references, their generated SDK and
OpenAPI types, and the openai/codex source. Every claim below carries a verbatim quote
and the URL it came from. Two supporting sources that are not vendor docs are labelled
as such: a first-party repository issue on openai/codex, and this relay's own live
`usage.json` counters (a captured wire artifact, which is evidence about what the relay
received, not about what a vendor documents).

The question this answers: **for each Provider, which field carries the cache counts, and
does that field sit inside the input count or beside it?** The relay records four
counters per account — `input`, `output`, `cache_read`, `cache_write` — and sums them as
"carried load" (`src/usage_view.rs:786`). That sum is only correct when the four are
disjoint.

## The two conventions

There is no universal convention. The vendors split into two camps, and the relay's
counters are built for the first:

- **Disjoint** — the input counter holds only tokens *not* served from or written to the
  cache, and the three counters partition the prompt. **Anthropic** and **DeepSeek**.
- **Subset** — the input counter is the whole prompt, and the cache counters are a slice
  of it. **OpenAI** (both dialects), **Codex**, **xAI Grok**, **Groq**, **OpenRouter**,
  **Moonshot/Kimi**, **Gemini**.

Summing `input + cache_read + cache_write` is correct in the first camp and double counts
the cache read in the second.

## Anthropic — disjoint

`https://docs.claude.com/en/docs/build-with-claude/prompt-caching`

> `cache_creation_input_tokens`: Number of tokens written to the cache when creating a new
> entry.
> `cache_read_input_tokens`: Number of tokens retrieved from the cache for this request.
> `input_tokens`: Number of input tokens which were **not read from or used to create a
> cache** (that is, tokens after the last cache breakpoint).

> The `input_tokens` field represents only the tokens that come **after the last cache
> breakpoint** in your request - not all the input tokens you sent. To calculate total
> input tokens: `total_input_tokens = cache_read_input_tokens +
> cache_creation_input_tokens + input_tokens`

The 1h breakdown is a nested object **beside** the flat field, not a replacement for it:

> Note that the current `cache_creation_input_tokens` field equals the sum of the values
> in the `cache_creation` object.

Confirmed in the vendor's own SDK types, which also carry fields the relay ignores:

```python
# anthropics/anthropic-sdk-python, src/anthropic/types/usage.py
cache_creation: Optional[CacheCreation] = None   # "Breakdown of cached tokens by TTL"
cache_creation_input_tokens: Optional[int] = None # "used to create the cache entry"
cache_read_input_tokens: Optional[int] = None     # "read from the cache"
input_tokens: int
output_tokens_details: Optional[OutputTokensDetails]  # thinking_tokens
service_tier: Optional[Literal["standard", "priority", "batch"]]
```

`https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/cache_creation.py`

```python
class CacheCreation(BaseModel):
    ephemeral_1h_input_tokens: int  # "used to create the 1 hour cache entry"
    ephemeral_5m_input_tokens: int  # "used to create the 5 minute cache entry"
```

Two further facts the docs state, both relevant to a counter that wants to mean something:

- **A usage array exists that the relay does not read.** The pre-warm example returns
  `usage.iterations[]`, each entry carrying its own `input_tokens`,
  `output_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens` and
  `cache_creation`. Ignoring it is correct — summing it would multiply every counter — but
  nothing in the relay pins that.
- **Pricing multipliers:** 5-minute cache writes 1.25× base input, 1-hour writes 2×, cache
  reads 0.1× — *"Cache read tokens are 0.1 times the base input tokens price (see the table
  footnote for per-model exceptions)"*, the exceptions being *"0.025x the base input price"*
  on Claude Fable 5.1 and Claude Mythos 5.1.

## OpenAI, both dialects — subset

`https://developers.openai.com/api/docs/guides/prompt-caching` shows a Responses exchange
where the input count is the total and the two cache fields partition it:

> `"input_tokens": 12000, "input_tokens_details": { "cached_tokens": 0,
> "cache_write_tokens": 12000 }`
> `"input_tokens": 15000, "input_tokens_details": { "cached_tokens": 12000,
> "cache_write_tokens": 3000 }`

> Cache-write pricing is not an additive fee: input tokens use the uncached-input,
> cached-input, or cache-write rate.

The vendor's generated types state the containment directly, and put `cache_write_tokens`
in **both** dialects:

`https://github.com/openai/openai-python/blob/main/src/openai/types/completion_usage.py` (Chat)

```python
class PromptTokensDetails(BaseModel):
    cache_write_tokens: Optional[int] = None  # "The unadjusted number of prompt tokens written to cache."
    cached_tokens: Optional[int] = None       # "Cached tokens present in the prompt."
class CompletionUsage(BaseModel):
    prompt_tokens: int      # "Number of tokens in the prompt."
    total_tokens: int       # "Total number of tokens used in the request (prompt + completion)."
```

`https://github.com/openai/openai-python/blob/main/src/openai/types/responses/response_usage.py`

```python
class InputTokensDetails(BaseModel):
    cache_write_tokens: int  # "The number of input tokens that were written to the cache."
    cached_tokens: int       # "The number of tokens that were retrieved from the cache."
```

`cached_tokens` and `cache_write_tokens` are *"present in the prompt"* / a *"breakdown of
the input tokens"*. `total_tokens` is prompt + completion, so it includes the cached slice
once, not twice.

## Codex (ChatGPT subscription backend) — subset, and the vendor's client does the subtraction

Wire path, in the words of a first-party repository issue:

`https://github.com/openai/codex/issues/32479`

> GPT-5.6 Responses report cache writes in `usage.input_tokens_details.cache_write_tokens`,
> but Codex currently drops this field while parsing usage.
> … Cache reads can be measured through `cached_input_tokens`, but cache writes cannot be
> measured even though **GPT-5.6 cache writes are billed at 1.25x the uncached input-token
> rate**.

The issue is closed as completed, and the current source carries the field:

`https://github.com/openai/codex/blob/main/codex-rs/protocol/src/protocol.rs`

```rust
pub struct TokenUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    #[serde(default)]
    pub cache_write_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}
impl TokenUsage {
    pub fn cached_input(&self) -> i64 { self.cached_input_tokens.max(0) }
    pub fn non_cached_input(&self) -> i64 { (self.input_tokens - self.cached_input()).max(0) }
    /// Primary count for display as a single absolute value: non-cached input + output.
    pub fn blended_total(&self) -> i64 { (self.non_cached_input() + self.output_tokens.max(0)).max(0) }
}
```

`non_cached_input = input_tokens - cached_input_tokens` is the vendor's own client doing the
exact subtraction this relay omits. `blended_total` additionally shows the convention: the
displayed total is *non-cached* input plus output, not the raw input count.

## xAI Grok — subset, with reasoning outside the completion count

`https://docs.x.ai/docs/api-reference` (OpenAPI schema)

```json
"input_tokens_details": { "cached_tokens": { "description":
  "Token cached by xAI from previous requests and reused for this request." } }
"output_tokens_details": { "reasoning_tokens": { "description":
  "Tokens generated by the model for reasoning." } }
```

The chat dialect nests it the OpenAI way, and the docs' own example shows
`prompt_tokens_details: { "text_tokens": 32, "audio_tokens": 0, "image_tokens": 0,
"cached_tokens": 8 }`.

**A quirk that matters for the relay's totals.** The same chat example reports
`prompt_tokens: 32`, `completion_tokens: 9`, `completion_tokens_details.reasoning_tokens:
110`, `total_tokens: 151`. 32 + 9 + 110 = 151 exactly: for this dialect xAI counts
reasoning **outside** `completion_tokens` while still counting it inside `total_tokens`.
The relay's `carried_tokens` deliberately excludes reasoning on the stated grounds that it
*"is already inside output"* (`src/usage_view.rs:782`) — true for OpenAI and Anthropic,
false here. The live grok pool is consistent with the docs: `output` 93 against
`reasoning` 1,236.

## DeepSeek — disjoint, and no `cached_tokens` at all

`https://api-docs.deepseek.com/guides/kv_cache`

> In the response from the DeepSeek API, we have added two fields in the `usage` section to
> reflect the cache hit status of the request:
> `prompt_cache_hit_tokens`: The number of tokens in the input of this request that
> resulted in a cache hit.
> `prompt_cache_miss_tokens`: The number of tokens in the input of this request that did
> not result in a cache hit.

The two are the halves of one input — a hit and a non-hit — so they read as a partition
rather than a slice. **Inferred, not quoted:** no sentence on that page states that the two
sum to `prompt_tokens`. The relay reads the hit as the cache read and subtracts it from
`prompt_tokens`, which reproduces the miss only while that sum holds; a vendor whose two
fields did not sum to the prompt count would be recorded wrongly, and that is the one
assumption in the DeepSeek row. That page contains no occurrence of `cached_tokens`, so the
relay reads nothing for a DeepSeek endpoint *except* through this pair.

## Groq — subset, stated as a hit rate

`https://console.groq.com/docs/prompt-caching`

> `prompt_tokens`: Total number of tokens in your input prompt
> `cached_tokens`: Number of input tokens that were served from cache (within
> `prompt_tokens_details`)
> `total_tokens`: Sum of prompt and completion tokens

> Cache Hit Rate = cached_tokens / prompt_tokens × 100%
> For the example above: `4608 / 4641 × 100% = 99.3%`

The vendor's own hit-rate formula puts the cached count over the *total* prompt, which is
the containment stated as arithmetic. The example response is
`prompt_tokens: 4641, completion_tokens: 1817, total_tokens: 6458,
prompt_tokens_details.cached_tokens: 4608`. Groq has **no cache-write field and no cache
write fee**: *"There is a 50% discount for cached input tokens"*, *"Prompt caching is
provided at no additional cost"*. Minimum cacheable length *"varies by model, ranging from
128 to 1024 tokens"*; entries expire after 2 hours.

## OpenRouter — subset, exposes reads **and writes** in the chat dialect

`https://openrouter.ai/docs/use-cases/usage-accounting`

> `cached_tokens` is the number of tokens that were *read* from the cache.
> `cache_write_tokens` is the number of tokens that were *written* to the cache (only
> returned for models with explicit caching and cache write pricing).

Example body: `"prompt_tokens": 194, "prompt_tokens_details": { "cached_tokens": 0,
"cache_write_tokens": … }`. This is the third-party confirmation that `cache_write_tokens`
exists in the **Chat Completions** shape, not only in Responses.

## Moonshot/Kimi — subset, and the field is **flat**

`https://platform.moonshot.ai/docs/api/chat` (OpenAPI schema, quoted verbatim from the
generated spec)

```json
"cached_tokens": { "type": "integer",
  "description": "Number of tokens served from cache",
  "uniqueKey": "usage.cached_tokens" }
```

`cached_tokens` is a **top-level sibling of `prompt_tokens` inside `usage`**. There is no
`prompt_tokens_details` in the schema at all — the string does not occur on that page. The
schema declares `usage` as *"Object in the final chunk with usage, null in ordinary
chunks"*, and the streamed example is
`usage: {prompt_tokens: 19, completion_tokens: 13, total_tokens: 32, cached_tokens: 12}` —
`19 + 13 = 32`, so `cached_tokens` is inside the prompt.

## Gemini — subset in the native API; compat surface unverified

`https://googleapis.github.io/java-genai/javadoc/com/google/genai/types/GenerateContentResponseUsageMetadata.html`

> `promptTokenCount`: The total number of tokens in the prompt. This includes any text,
> images, or other media provided in the request. **When `cached_content` is set, this also
> includes the number of tokens in the cached content.**

> `cachedContentTokenCount`: Output only. The number of tokens in the cached content that
> was used for this request.

Inclusive, explicitly. The relay never speaks this API directly, and Google's
OpenAI-compatibility page (`https://ai.google.dev/gemini-api/docs/openai`) documents no
cache field at all, so what a Gemini endpoint registered as a generic provider returns is
**unverified**.

## What the relay read when this was researched

Function names and no line numbers, deliberately: every one of these functions moved when
ADR-0023 landed, and the readers below are the *nested-only* ones this research found. What
replaced them is `cache_read_from`, `cache_write_from` and `output_from` in `src/app.rs`, with
the subtraction in `separate_cache_from_input` at `record_provider_success`; a vendor whose
name is added to this file belongs in `cache_read_from` / `cache_write_from` too, which is how
ADR-0023 states the same risk.

```
src/app.rs  usage_from_response        JSON, every ProviderKind
                 input_tokens | prompt_tokens
                 cache_read_input_tokens
                 | input_tokens_details.cached_tokens
                 | prompt_tokens_details.cached_tokens
                 cache_creation_input_tokens
                 cache_creation.ephemeral_1h_input_tokens
src/app.rs  update_generic_stream_usage   Grok + generic streams
                 prompt_tokens
                 prompt_tokens_details.cached_tokens        <- nested only
                 completion_tokens_details.reasoning_tokens <- nested only
src/app.rs  update_codex_stream_usage     -> usage_from_response (nested only)
src/streaming.rs update_chat_usage        same nested pair only
```

Against the vendors above. The last column is what the relay recorded **at research time** —
the gap this file exists to record; ADR-0023 closed it, and the code that closes it is named
above.

| Provider | Cache read | Cache write | Reasoning | Recorded at research time |
|---|---|---|---|---|
| Anthropic | `cache_read_input_tokens` (disjoint) | `cache_creation_input_tokens` + nested 1h | `output_tokens_details.thinking_tokens` | **yes** |
| OpenAI Chat | `prompt_tokens_details.cached_tokens` | same object, `cache_write_tokens` | nested | read yes, write **no**; input double counts |
| OpenAI Responses | `input_tokens_details.cached_tokens` | `input_tokens_details.cache_write_tokens` | nested | read yes, write **no**; input double counts |
| Codex | `input_tokens_details.cached_tokens` | `input_tokens_details.cache_write_tokens` | nested | read yes, write **no**; input double counts |
| xAI Grok | `input_tokens_details.cached_tokens` / `prompt_tokens_details.cached_tokens` | none documented | nested | read yes; input double counts; reasoning dropped from carried load |
| DeepSeek | `prompt_cache_hit_tokens` | none | — | **no — reads 0** |
| Groq | `prompt_tokens_details.cached_tokens` | none (no write fee) | nested | read yes; input double counts |
| OpenRouter | `prompt_tokens_details.cached_tokens` | `prompt_tokens_details.cache_write_tokens` | nested | read yes, write **no**; input double counts |
| Moonshot/Kimi | `usage.cached_tokens` (**flat**) | none documented | `reasoning_content` in the message, not usage | **no — reads 0** |
| Gemini (compat) | unverified | unverified | unverified | unknown |

## Live counter evidence

Read from `~/.pengepul/*/usage.json` on the development machine, 2026-09-12:

```
anthropic     input      3,811,609   read  1,266,186,199   write 65,688,261
commandcode   input    514,634,436   read    498,866,688   write          0
grok          input         58,908   read         15,104   write          0
openrouter    input         17,651   read              0   write          0
```

- **anthropic** is the disjoint signature: `input` is a rounding error beside the reads,
  which is what "tokens after the last breakpoint" looks like.
- **commandcode** (an OpenAI-dialect generic provider) is the opposite: `input` 514.6M
  against `read` 498.9M. Under subset semantics the uncached remainder is ~15.7M; under
  disjoint semantics the prompt would be 1.01B. The near-equality of the two figures is the
  double count in the wild, and it is the shape ADR-0017's "cache share of input tokens"
  table was computed on.
- **grok** records 15,104 cache reads, which **refutes** this repository's own note at
  `docs/research/grok-build-provider.md:76` claiming Grok's `cached_tokens` is flat. A flat
  field could not have been parsed by the nested-only reader, so the proxy reports the
  nested shape.

## Corrections to this repository's earlier claims

1. `docs/research/grok-build-provider.md:76` — *"`usage` includes `reasoning_tokens`,
   `cached_tokens`"* — read as flat fields, is **refuted** by both the xAI schema (nested
   under `prompt_tokens_details` / `input_tokens_details`) and the live grok counters.
   Grok's real unhandled gap is different: reasoning outside `completion_tokens`.
2. The flat-field gap is real but belongs to **Moonshot/Kimi** (`usage.cached_tokens`),
   which no prior note names.
3. ADR-0017's Context table reports a "Cache share of input tokens" per pool. That
   ratio is only a hit rate where the input count excludes the cache — Anthropic and
   DeepSeek. Everywhere else the denominator already contains the numerator, so those
   figures were not hit rates, and the metric cannot be compared across pools. ADR-0023
   fixed what the counters mean; the table now carries a pointer saying not to reuse it.

## Gaps

- **Gemini's OpenAI-compatible usage shape**: not documented on Google's compat page and not
  captured here. Needs one real response body.
- **Whether any of these vendors reports a cache-write field the relay should map**: only
  OpenAI, Codex and OpenRouter are documented as doing so; DeepSeek, Groq and Moonshot are
  documented as having none.
- **Whether DeepSeek's `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens` sum to
  `prompt_tokens`**: the field descriptions imply it; no sentence states it, and the relay's
  subtraction depends on it. One captured DeepSeek response body settles it.
- **Streaming cumulativity per vendor**: Moonshot's schema shows usage only on the final
  chunk; Groq's example is a final chunk; OpenAI chat requires `stream_options.include_usage`.
  The relay treats the chat value as last-wins cumulative, which matches all three, but no
  vendor page was found that says the counts are cumulative rather than per-chunk for a
  multi-chunk stream.
