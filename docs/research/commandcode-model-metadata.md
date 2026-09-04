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
