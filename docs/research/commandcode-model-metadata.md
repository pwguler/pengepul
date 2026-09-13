# Commandcode-family model metadata, from vendor docs

Researched 2026-09-04, to extend the curated per-model metadata in
`src/models.rs` (`curated_metadata`) past the claude families to the models the
commandcode relay serves. commandcode's own `/v1/models` publishes only
`context_length`, so `max_output_tokens` and `reasoning` come from the vendors.
The merge is per-field: what commandcode publishes (context) wins, the curated
entry fills the rest.

## Sourced families

- DeepSeek (pricing + model details): https://api-docs.deepseek.com/quick_start/pricing
  — v4-flash/pro/flash-vision-exp: 1M context, 384K max output, thinking mode
  default on. Rates below are the peak tier (off-peak is half). flash-fast is
  covered by the family default.
- Moonshot (K3/K2.7 pricing + guides): https://platform.moonshot.ai/docs/pricing/chat-k3.md,
  /docs/pricing/chat-k27-code.md — K3: 1M context, visual input, thinking
  (effort default max). K2.7-Code/-Highspeed: 256K context, thinking mode.
  Max output is not published.
- Z.AI (model pages + pricing): https://docs.z.ai/guides/llm/glm-5.3.md,
  /glm-5.2.md, /glm-5.1.md, /vlm/glm-5.3-flash.md — GLM-5.3/5.2: 1M context,
  128K max output; GLM-5.1: 128K max output; GLM-5.3 always reasons (disabling
  removed); GLM-5.3-Flash too (`thinking.type` supports `enabled` only).
  GLM-5.1/5 context per commandcode. Pricing is promotion-dependent, so no
  pricing is carried.
- MiniMax (models overview): https://platform.minimax.io/docs/guides/models-intro.md
  — M3: 1M context, multimodal; M2.7/M2.5 in current lineup. `reasoning` rides
  the M-series thinking lineage (M2.x pages state enhanced reasoning); M3's own
  page does not spell out thinking.
- xAI (model pages): https://docs.x.ai/docs/models/grok-4.5.md and /grok-4.6.md
  — both 500,000 context, reasoning yes, text+image. Pricing is tiered at
  200k prompt tokens; the table carries the base (<200k) tier.
- OpenAI (model page): https://platform.openai.com/docs/models/gpt-5.6.md
  — gpt-5.6 (Sol alias): 1,050,000 context, 128,000 max output, text+image,
  reasoning token support. Input $4 / cached $0.4 / output $20 is promotional
  pricing (through Nov 21, 2026).
- Qwen: the Qwen3 family ships hybrid thinking; per-model limits for 3.8/3.7/3.6
  were not published on the pages fetched, so only `reasoning` is carried.
- Google: Gemini 3 flash models support thinking (Gemini API models page,
  https://ai.google.dev/gemini-api/docs/models); per-model token limits were not
  in the fetched page, so only `reasoning` is carried.

## Families deliberately left bare

No vendor documentation was found in this pass for: `xiaomi/mimo-v2.5(-pro)`,
`stepfun/Step-{3.7,3.5}-Flash`, `tencent/hy{3-paid,4-preview}`,
`meituan/LongCat-2.0:free`, `meta/muse-spark-1.{1,2,3}(-contributor)`,
`nvidia/nemotron-3-ultra-550b-a55b`, `thinkingmachines/inkling(-small)`,
`poolside/laguna-s-2.1-free`, `sakana/fugu-ultra`. Those ids stay without
curated metadata (context still passes through from commandcode); a client
falls back to its own catalog for them. Extending the table later is a data
change, not a code change.

## Input modalities, measured on the wire

Researched 2026-09-14 (pengepul 0.102.0), after issue #8 reported that
`input_modalities` was wrong in both directions. commandcode's `/v1/models`
publishes only `context_length`, so every modality value the relay advertises
comes from `PROVIDER_MODALITIES` or the family tables in `src/models.rs` — there is
no upstream field to pass through.

Method. A 256x256 PNG in four 128px quadrants — top-left olive, top-right teal,
bottom-left maroon, bottom-right navy — sent as a data URL, and a question that
needs the image: name the colour of each quadrant, or say which quadrant holds a
named colour. A blind model can only guess, one quadrant in four per question,
and must also name the colour; a text-only model does not fail loudly, it answers
with a confident wrong quadrant (`deepseek/deepseek-v4-pro` did exactly that) or
says it cannot see an image. `max_tokens` must be generous: a reasoning model
spends the front of the budget on `reasoning_content` and returns empty content
with `finish_reason: "length"`, which reads like a vision failure and is not one.

| id | route | advertised before | measured |
| --- | --- | --- | --- |
| `deepseek/deepseek-v4.1-flash` | commandcode | `["text"]` | reads images (two questions) |
| `deepseek/deepseek-v4.1-flash` | openrouter | `["text"]` | reads images (4/4 quadrants) |
| `deepseek/deepseek-v4-flash` | commandcode | `["text"]` | reads images |
| `deepseek/deepseek-v4-flash` | openrouter | `["text"]` | **refuses** — `404 No endpoints found that support image input` |
| `deepseek/deepseek-v4-flash-fast` | commandcode | `["text"]` | text-only, and says so: *"no image was provided in your message"* |
| `deepseek/deepseek-v4-pro` | commandcode | `["text"]` | text-only, and says so: *"the image is unsupported"* |
| `deepseek/deepseek-v4-pro`, `-pro-0813`, `-flash-0731` | openrouter | `["text"]` | **refuse** — `404 No endpoints found that support image input` |
| `deepseek/deepseek-v4-pro-0813:batch` | openrouter | `["text"]` | unmeasured — the account was cooled by the 404s above before it answered |
| `deepseek/deepseek-v4-flash-0731:batch` | openrouter | `["text"]` | not served over chat at all: *"only available through the Batch API"* |
| `google/gemini-3.8-flash` | commandcode | absent | reads images |
| `xiaomi/mimo-v2.5` | commandcode | absent | reads images |
| `z-ai/glm-5.3-flash` | commandcode | absent | reads images |

One half of the problem is left open. The family tables claim
`["text","image"]` for 21 live commandcode ids (`claude-*`, `gpt-*`,
`moonshotai/Kimi-*`, `xai/grok-*`) from vendor documentation rather than from a
measurement against the Provider serving them, and the OpenRouter result shows
that inference can be wrong per Provider. Those claims were not measured and stay
as they are. A false positive is the more expensive direction — a client attaches
an image the upstream refuses, which on OpenRouter is the `404` above, booked as a
`network` failure and a cooldown.

Two conclusions the table now encodes:

1. **A modality claim is a claim about a route, not about a model name.**
   `deepseek/deepseek-v4-flash` reads images through commandcode and refuses them
   through OpenRouter, so no table keyed by the bare id can be right for both.
   `PROVIDER_MODALITIES` names the Provider each measured claim was measured
   against, and matches exactly rather than by prefix, so the claim cannot leak
   onto `deepseek-v4-flash-fast`, which refuses images. An entry naming no
   Provider was measured against every Provider pengepul was pointed at — two
   here, commandcode and OpenRouter — not against every Provider that could be
   configured; the same upstream registered under a new name gets no claim until
   it is measured under that name.
2. **An unmeasured `["text"]` is left alone.** The family tables still assert
   `["text"]` for ids nothing has measured; that is a claim pengepul has not
   refuted, and it is recorded here rather than silently weakened.

   Omitting the field instead is not the cheap fix it looks like, and the reason
   is not the one this note first gave. The client decides: the
   `pi-pengepul-provider` extension looks a relay id up in pi's own catalogs by
   full id, then by last segment, then by namespace-stripped id, and a builtin
   entry wins over its heuristic, which always answers `["text"]`
   (`extensions/models.ts:109-141,171`). Verified 2026-09-14 against the installed
   catalogs: `commandcode/google/gemini-3.8-flash` resolves to `gemini-3.8-flash`
   (github-copilot, `["text","image"]`), `commandcode/xiaomi/mimo-v2.5` to
   `mimo-v2.5` (opencode-go, `["text","image"]`) and `commandcode/z-ai/glm-5.3-flash`
   to `glm-5.3-flash` (opencode, `["text","image"]`) — so for those three, omitting
   the field would have restored the image through the client's own catalog.
   `commandcode/deepseek/deepseek-v4.1-flash` matches no catalog entry and would
   fall to the heuristic, staying text-only. Advertising the measured truth is
   therefore what fixes all six ids, for every client rather than for pi alone.
   `dist/core/provider-composer.js`'s `input ?? ["text"]` is not the deciding
   layer: the provider always ships an `input`.

Cost of the measurement: 23 requests in four runs (7 + 2 + 9 + 5), plus one
   account cooldown on the OpenRouter pool. Attaching an
image to a text-only id on OpenRouter earns a `404`, which pengepul books as a
`network` failure, so the account cools down and the next requests answer
`503 no available openrouter account; last failure: network`. A client that
trusts an advertised `["text", "image"]` for a model that has none therefore
costs more than a dropped image, which is why the measured positives here are
scoped to the route that produced them.
