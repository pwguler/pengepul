# 14. Cloaking mirrors Claude Code's headers, except where fidelity breaks the client

Status: Accepted

## Context

pengepul presents subscription traffic as a first-party Claude Code request:
User-Agent, Stainless fingerprint, and the `anthropic-beta` set are copied
from the real CLI so the billing classifier accepts the call (ADR-0012 learns
the version at runtime). The natural maintenance move is therefore "diff our
beta list against the current Claude Code binary and sync it".

One flag in that list turned out to be self-inflicted damage. Claude Code
2.1.261 sends `redact-thinking-2026-02-12` when it runs interactively with
thinking summaries hidden — because its own TUI does not show thinking, it
asks the server not to send the text. The server then returns every
`thinking` block with an empty string (signature intact, `thinking_tokens`
still billed). Relayed to pi or openclaw, which do render thinking, the
symptom was "Claude never thinks": 68 assistant turns in one session, 9 with
a thinking block, all 0 characters, thousands of thinking tokens billed.

A/B through the relay, identical request, only the beta set differing:

| beta set | thinking text | thinking_tokens |
|---|---|---|
| with `redact-thinking` | 0 chars | 118 |
| without | 197 chars | 117 |

## Decision

The cloaking header set is **based on** the current Claude Code, not a copy
of it. Two deliberate deviations, both documented next to the list in
`build_beta_header` and in `docs/research/claude-code-2.1.261-cloaking-audit.md`:

- `redact-thinking-2026-02-12` is **never sent**. It encodes a Claude Code
  UI preference, not a protocol requirement, and it destroys the thinking
  text pengepul's clients display.
- `web-fetch-2025-09-10` is **kept** although 2.1.261 no longer sends it:
  the native `web_fetch_20250910` server-tool swap (ADR-0008) requires it.

Everything else follows the binary: `thinking-token-count-2026-05-13` added,
Stainless SDK `0.112.1` / runtime `v26.3.0`, `anthropic-client-platform`.

## Consequences

- Thinking text flows to pi/openclaw; `thinking_tokens` now also feeds the
  per-account reasoning counter shown by `status`.
- A future audit against a newer Claude Code must re-read this ADR before
  syncing: `redact-thinking` reappearing is a regression, guarded by a test
  that pins the full beta string.
- Fidelity to the real client is slightly lower (one flag missing, one
  extra). No classifier rejection was observed with either deviation; if one
  ever appears, the trade-off is re-decided here, not by silently restoring
  the flag.
