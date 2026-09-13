# input-modalities-per-provider

## Goal

`GET /v1/models` advertises `input_modalities` that matches what the upstream behind each id
actually accepts, per **Provider**. Issue #8: two ids were advertised `["text"]` while reading
images, and three capable ids carried no field at all, so a client that gates its image path on
the field dropped images the model could read.

The root of it is that the field is hand-written. commandcode's `/v1/models` publishes only
`context_length`, so every modality value comes from the tables in `src/models.rs`.

## Shape

- `PROVIDER_MODALITIES` holds one entry per **measured** claim: the bare model id, the
  Provider the measurement was against (`None` when it was measured against every Provider
  pengepul was pointed at), and the modalities.
- `curated_metadata(id, provider)` consults it after the family tables: a matching entry
  replaces `input_modalities` and leaves the rest of the family metadata alone. An id the
  family tables never claimed gets modalities as its only metadata, rather than invented
  limits or zeroed rates.
- The match is **exact**, not by prefix: `deepseek/deepseek-v4-flash` must not carry its claim
  onto `-flash-fast`, which refuses images.
- The provider thread runs through `parse_openai` / `parse_anthropic` / `parse_codex`, which
  take the Provider the body came from.
- Upstream metadata still wins: a Provider that publishes its own `input_modalities` overrides
  a claim here (`merge_curated`), so these claims hold while it stays silent.

## Non-goals

- **No measurement of the family tables' `["text","image"]` claims.** 21 live commandcode ids
  inherit image support from vendor documentation rather than from a measurement against that
  Provider. They are unchanged, and the gap is recorded in
  `docs/research/commandcode-model-metadata.md`.
- **No change to the `/v1/models` shape**, no new field, no change to routing, bodies, config
  or the CLI.
- **No per-Provider split of the family tables.** Only the ids measured differently per Provider
  move into the new table.
- **No inference from model names.** A colon or a `vision` in an id is not evidence; the two
  ids in this change were measured.
- **No weakening of unmeasured `["text"]` claims to omission.** That was considered and
  declined; the research note records why it is not the cheap fix it looks like.

## Acceptance criteria

- AC-1: `parse_openai` with `deepseek/deepseek-v4-flash` yields `["text","image"]` for a
  commandcode Provider and `["text"]` for an OpenRouter one, from the same body.
- AC-2: `deepseek/deepseek-v4.1-flash` yields `["text","image"]` for both, since it was measured
  against both.
- AC-3: The exact match holds: `deepseek-v4-flash-fast`, `-flash-0731`, `deepseek-v4-pro` and
  `-pro-0813` keep `["text"]` on both Providers.
- AC-4: `google/gemini-3.8-flash`, `xiaomi/mimo-v2.5` and `z-ai/glm-5.3-flash` advertise
  `["text","image"]` on commandcode and **nothing** on an unmeasured Provider.
- AC-5: The advertised payload — `advertised()` from a catalog built by `parse_openai` — carries
  those values under the prefixed id, so the fix reaches the HTTP seam rather than only the parse.
- AC-6: The shipping path carries the Provider: driving the real `HttpUpstreamClient::fetch_models`
  twice against one local `/v1/models` body, once per Provider, yields the two different claims.
  A Provider substituted at that call site fails this.
- AC-7: No claude, gpt or grok id changes claim, and the anthropic, codex and grok catalogs are
  byte-identical to before the change.
- AC-8: An id with no family entry gains `input_modalities` and nothing else — no invented
  `context_window`, `max_output_tokens` or `pricing`.

## Verification

```sh
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
```

AC-1..AC-4 are `a_provider_scoped_claim_holds_only_on_the_provider_it_was_measured_on` and
AC-5 is `the_advertised_payload_carries_the_measured_modalities`, both in `src/models.rs`.
AC-6 is `the_fetch_asks_each_provider_for_its_own_modality_claims` in `src/app.rs`, which binds
a loopback port and drives the real fetch. AC-7 is the unchanged family tables plus the
existing catalog tests. Each is verified by mutation: dropping the Provider scope, switching
the match to a prefix, removing an entry, and hard-coding a Provider at the fetch call site each
fail a test.
