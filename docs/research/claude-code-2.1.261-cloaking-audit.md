# Claude Code 2.1.261 cloaking audit

Source: the published native binary `@anthropic-ai/claude-code-linux-x64@2.1.261`
(built 2026-09-04), `strings` + reading the beta registry (`me("name","header")`)
and the per-request assembler `Vre(model)`.

## Beta flags a first-party request carries

| flag | condition in 2.1.261 | relay |
|---|---|---|
| `claude-code-20250219` | non-haiku | sends |
| `oauth-2025-04-20` | OAuth subscription auth | sends |
| `interleaved-thinking-2025-05-14` | thinking-capable model | sends |
| `thinking-token-count-2026-05-13` | first-party + thinking model | **added** (usage gets `thinking_tokens`) |
| `redact-thinking-2026-02-12` | interactive TUI **and** `showThinkingSummaries=false` | **removed** — it empties every `thinking` block; a relay serving pi/openclaw must not send it |
| `context-management-2025-06-27` | feature-gated | sends |
| `prompt-caching-scope-2026-01-05` | first-party | sends |
| `advanced-tool-use-2025-11-20` | first-party (`tool-search-tool` elsewhere) | sends (non-haiku) |
| `effort-2025-11-24` | only when an `effort` param is set | sends always (accepted; over-sends) |
| `structured-outputs-2025-12-15` | gated + structured request | sends only when structured |
| `context-1m-2025-08-07` | model has 1M context | not sent |
| `web-fetch-2025-09-10` | **not present in 2.1.261** | kept: needed by the native `web_fetch` server-tool swap |
| `web-search-2025-03-05` | vertex/foundry only | not sent |

## Headers

| header | 2.1.261 | relay before | relay now |
|---|---|---|---|
| `User-Agent` | `claude-cli/<ver> (external, cli)` | same shape, version auto-tracked | unchanged |
| `X-Stainless-Package-Version` | `0.112.1` (bundled `@anthropic-ai/sdk`) | `0.74.0` | `0.112.1` |
| `X-Stainless-Runtime-Version` | `v26.3.0` (bun node-compat) | `v22.13.0` | `v26.3.0` |
| `X-Stainless-Runtime`/`Lang`/`Arch`/`OS` | node / js / arch / os | same | unchanged |
| `anthropic-client-platform` | new; the entrypoint (`cli`) | absent | added |
| `x-app`, `anthropic-version`, `anthropic-dangerous-direct-browser-access` | as before | same | unchanged |

## Verified effect

Same request through the relay, before vs after: `thinking` block text
0 chars → 224 chars (non-stream, `budget_tokens`), 0 → 59 chars streamed
(`adaptive` + `effort: max`, pi's shape); `thinking_tokens` billed the same.
