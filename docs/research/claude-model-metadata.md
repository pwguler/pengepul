# Claude model metadata, from vendor docs

Researched 2026-09-02, to settle the curated per-model metadata in
`src/models.rs` (`curated_metadata`) after the opus-5 entry was disputed.
The anthropic `/v1/models` endpoint advertises ids only (`id`, `display_name`,
`created`), so the vendor docs are the primary source.

## Sources

- Models overview (comparison table):
  https://platform.claude.com/docs/en/models/overview
- Pricing (model pricing table + caching multipliers):
  https://platform.claude.com/docs/en/about-claude/pricing
- Per-model pages, e.g. https://platform.claude.com/docs/en/models/opus-5/overview
  (same shape for opus-4-5/4-6/4-7/4-8 and sonnet-4-5/4-6)

## Findings

Context window / max output (models overview):

| model | context | max output |
|---|---|---|
| claude-fable-5-1 | 1M | 128K |
| claude-opus-5 | 1M | 128K |
| claude-sonnet-5 | 1M | 128K |
| claude-haiku-4-5 | 200K | 64K |
| claude-opus-4-8 | 1M | 128K |
| claude-opus-4-7 | 1M | 128K |
| claude-opus-4-6 | 1M | 128K |
| claude-opus-4-5 | 200K | 64K |
| claude-sonnet-4-6 | 1M | 128K |
| claude-sonnet-4-5 | 200K | 64K |

Pricing, USD per MTok (pricing page; cache write is 5-minute):

| model | input | output | 5m cache write | cache read |
|---|---|---|---|---|
| claude-fable-5-1 | 10 | 50 | 12.50 | **0.25** |
| claude-opus-5 | **5** | **25** | **6.25** | **0.50** |
| claude-sonnet-5 | **2** | **10** | **2.50** | **0.20** |
| claude-haiku-4-5 | 1 | 5 | 1.25 | 0.10 |
| claude-opus-4-5 … 4-8 | 5 | 25 | 6.25 | 0.50 |
| claude-sonnet-4-6 | 3 | 15 | 3.75 | 0.30 |
| claude-sonnet-4-5 | 3 | 15 | 3.75 | 0.30 |

Caching multipliers (pricing page): 5-minute cache write 1.25x base input,
cache read 0.1x base input — except fable-5-1 (and mythos-5.1), whose cache
read is 0.025x. One-hour cache write is 2x; pengepul's `cache_write_per_million`
carries the 5-minute rate.

Notes:
- The old opus-4.1/4.0 rates ($15/$75, $1.50 read) are retired models; the
  original curated table had wrongly carried them onto opus-5.
- Sonnet 5's $2/$10 was launch-introductory but is now the standard price; the
  scheduled rise to $3/$15 was cancelled.
- claude-fable-5 (non-.1) keeps the standard 0.1x cache-read multiplier ($1).

## Outcome

The original curated table was wrong for claude-opus-5 (200K/64K/$15 instead of
1M/128K/$5), claude-sonnet-5 (64K output, $3 instead of 128K output, $2), and
fable-5-1's cache-read rate. Corrected in `src/models.rs`, and the opus-4.x and
sonnet-4.x models the relay serves were added, closing the "no metadata" gap
for them.
