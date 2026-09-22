# Claude Code 2.1.280 cloaking audit

Source: the native binary `@anthropic-ai/claude-code-linux-x64` 2.1.280 as installed at
`~/.local/share/claude/versions/2.1.280` (build `2026-09-21T20:40:17Z`, git `80abbfe7`).
The binary embeds its JS module graph, so the tables below are read from the CLI's own
code — the beta registry (`w(name, header)`), the per-request assembler (`oQt(model)`) and
the client's default headers (`QW(...)`) — rather than from `strings`. Compared against
2.1.261 (the same published `@anthropic-ai/claude-code-linux-x64` binary, build
`2026-09-04T16:49:50Z`, git `1349cf9c`) and the locally installed 2.1.277 and 2.1.278, all
four of which embed readable source.

All conditions below assume the first-party provider with a base URL the CLI considers its
own, which is what the relay presents.

## Beta flags a first-party request carries

| flag | condition in 2.1.280 | relay |
|---|---|---|
| `claude-code-20250219` | model is not haiku | sends (non-haiku) |
| `oauth-2025-04-20` | OAuth subscription scopes, or a resolvable credential | sends |
| `interleaved-thinking-2025-05-14` | thinking-capable model; on a first-party provider only `claude-3-*` is excluded | sends (not `claude-3-*`) |
| `thinking-token-count-2026-05-13` | same model gate; usage carries `thinking_tokens` | sends (not `claude-3-*`) |
| `context-management-2025-06-27` | first-party or Foundry, and not `claude-3-*` | sends (not `claude-3-*`) |
| `prompt-caching-scope-2026-01-05` | first-party | sends |
| `mid-conversation-system-2026-04-07` | model at or past `claude-opus-4-8`, or a model the CLI does not know | **added** |
| `redact-thinking-2026-02-12` | interactive TUI **and** `showThinkingSummaries=false` | **removed** (ADR-0014) |
| `structured-outputs-2025-12-15` | `output_format` requested and the model supports it | only with `output_format` or `output_config.format`, without the CLI's model check |
| `advanced-tool-use-2025-11-20` | tool search is on (tools deferred behind a tool-search tool) | only when the body carries a `tool_search_tool_*` type or a deferred tool |
| `effort-2025-11-24` | an `effort` value is set (`output_config.effort`) | only with `output_config.effort` |
| `task-budgets-2026-03-13` | the request carries a task budget | only with `output_config.task_budget` |
| `context-1m-2025-08-07` | the model id carries `[1m]` | not sent |
| `web-search-2025-03-05` | vertex or foundry only | not sent |
| `web-fetch-2025-09-10` | **not in 2.1.280 at all** | kept, see below |
| `inline-tools-2026-09-15` | CCR session **and** mid-conversation tool changes **and** the inline-tools gate | not sent |
| `per-turn-control-2026-07-01`, `timing-2026-09-09`, `mid-conversation-tool-changes-2026-07-01`, `mid-conversation-system-clear-at-2026-08-21`, `thinking-resumption-2026-07-17`, `message-threads-2026-08-12`, `dangerous-tool-use-2026-09-03`, `advisor-tool-2026-03-01` | per-turn timing, CCR, or a per-feature gate; none is on a plain request | not sent |

The assembler is declarative in 2.1.280 (a `when` predicate per flag); 2.1.261, 2.1.277 and
2.1.278 express the same gates as an if-chain. The always-on set is the same in all four,
which means the relay's two long-standing over-sends (`advanced-tool-use-2025-11-20`,
`effort-2025-11-24`) were never unconditional in any of them, and
`mid-conversation-system-2026-04-07` has been missing from the relay since at least 2.1.261.
The deltas marked below are relative to what the relay sent before this change.

A request whose own `User-Agent` starts with `claude-cli` keeps its `anthropic-beta`
verbatim — the relay only inserts `oauth-2025-04-20` when it is missing — so this set is
what the relay adds for every other client.

Two model lists drive the model gates. The CLI's generation test (`or(model, newer)`
over an ordered list of ten releases, oldest first:
`claude-opus-4-0`, `claude-sonnet-4-0`, `claude-opus-4-1`, `claude-sonnet-4-5`,
`claude-haiku-4-5`, `claude-opus-4-5`, `claude-opus-4-6`, `claude-sonnet-4-6`,
`claude-opus-4-7`, `claude-opus-4-8`) decides `mid-conversation-system`: a model listed
before `claude-opus-4-8`, or any `claude-3-*`, does not get it; a model the list does not
name does. The comparison is by family: a trailing release date and a `[1m]` marker are
ignored, so `claude-haiku-4-5-20251001` is the `claude-haiku-4-5` the list holds.
`interleaved-thinking` and `context-management` exclude only `claude-3-*` on a first-party
provider — haiku-4-5 keeps both.

### Registry delta

2.1.261 → 2.1.280 adds five entries and drops or renames none:
`thinking-resumption-2026-07-17`, `timing-2026-09-09`, `inline-tools-2026-09-15`,
`message-threads-2026-08-12`, `mid-conversation-system-clear-at-2026-08-21`. 2.1.277 and
2.1.278 already carry the other four, so from either of them the delta is
`inline-tools-2026-09-15` alone. That one is new to the always-considered set, and it is
gated behind a CCR session with mid-conversation tool changes, so no first-party request
from this relay's clients reaches it.

## Headers

| header | 2.1.280 | relay |
|---|---|---|
| `User-Agent` | `claude-cli/<ver> (external, <entrypoint>)`, plus optional `agent-sdk/`, `client-app/`, `workload/` suffixes | same shape, version auto-tracked |
| `X-Stainless-Package-Version` | `0.112.1` (bundled `@anthropic-ai/sdk`) | unchanged |
| `X-Stainless-Runtime-Version` | `v26.3.0` (bun node-compat) | unchanged |
| `X-Stainless-Lang`/`Runtime`/`Arch`/`OS` | node / js / arch / os | unchanged |
| `x-app` | `cli`, or `cli-bg` for a background session | `cli` |
| `X-Claude-Code-Session-Id` | per-session id | derived from the client key, stable across turns |
| `x-client-request-id` | uuid v4, per request | same |
| `anthropic-version` | `2023-06-01` | same |
| `anthropic-dangerous-direct-browser-access` | `true` (SDK `dangerouslyAllowBrowser`) | same |
| `anthropic-client-platform` | **not sent on Messages**; set only outside the Messages client, where the desktop app sends `desktop_app` plus `anthropic-client-version` | **removed** |
| `x-claude-code-request-class`, `x-claude-code-agent-type`, `anthropic-*` gateway hints | only with `CLAUDE_CODE_GATEWAY_HINT_HEADERS=1` or an SDK host | absent |

The 2.1.261 audit recorded `anthropic-client-platform: cli` as a header the CLI had started
sending. It does not: 2.1.261, 2.1.278 and 2.1.280 all build the Messages client's
default headers the same way (`{"x-app", "User-Agent", "X-Claude-Code-Session-Id", <custom
headers>, <container/session ids>}`), and every `anthropic-client-platform` site in those
builds belongs to a path that is not the Messages one — cloud sessions, the model selector,
the bridge, the voice stream, or the desktop app. The relay sent it and no longer does.

## Attribution header

The billing line is the first system block, followed by `You are Claude Code, Anthropic's
official CLI for Claude.` In 2.1.280 it is:

```
x-anthropic-billing-header: cc_version=<version>.<hash>; cc_entrypoint=<entrypoint>; cch=00000;
```

`cch=00000;` is a literal marker the CLI appends when the base URL is its own — the relay
sends it. `cc_workload`, `cc_is_subagent`, `cc_prev_req`, `cc_prompt_id` and
`cc_turn_origin` (present since at least 2.1.277) need a host to supply the value, which the
relay has no equivalent for, so all five stay out.

## Server tools

| type | 2.1.280 |
|---|---|
| `web_search_20250305` | present; the CLI's own beta table wires it to vertex and foundry only |
| `advisor_20260301` | present (2.1.276 fixed a 400 when a proxy received it without support) |
| `tool_search_tool_bm25`, `tool_search_tool_regex` | present, paired with `defer_loading` on deferred tools and `tool_search_tool_result` blocks |
| `clear_thinking_20251015`, `compact_20260112` | present |
| `web_fetch_20250910` | **gone**: WebFetch is a client-side tool in 2.1.280 (provenance set, local fetch, apply pass) |

The `web_fetch` swap in `masquerade` therefore presents a server tool the current CLI no
longer uses, and needs the `web-fetch-2025-09-10` beta to do it. It is kept — dropping it
would leave openclaw's `web_fetch` with no upstream implementation — and the beta is scoped
to requests that actually carry the swapped tool.

Claude Code's own tool names are unchanged in 2.1.280 (`Bash`, `Read`, `Edit`, `Write`,
`Glob`, `Grep`, `WebFetch`, `WebSearch`, `NotebookEdit`, `TaskCreate`, `TaskGet`,
`TaskUpdate`, `TodoWrite`, `LSP`), so `masquerade`'s `PascalCase` renaming still produces
the right shape.

## Verification

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The beta list is covered by `beta_header_follows_the_request_shape`,
`mid_conversation_system_follows_the_model_generation` and
`claude_3_drops_the_thinking_and_context_betas` in `tests/upstream.rs`; the header set by
`anthropic_headers_include_cloaking_session_and_beta`.
