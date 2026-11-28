# output-ceiling

## Goal

A request that names no output limit is answered as far as the model can go, not cut at a
number the relay made up. The Messages dialect requires `max_tokens`, so a Chat Completions
or Responses request translated onto the anthropic **Upstream** must carry one; the relay
filled it with `8192`, and an opus reply with thinking stopped at exactly 8192 output tokens
(`finish_reason=length`). Hermes sends its context-compression summary that way on purpose,
so compression of a large session never completed.

The reasoning and the rejected alternatives are in ADR-0035.

## Non-goals

- **No change to a client-set limit.** `max_tokens`, `max_completion_tokens` or Responses
  `max_output_tokens` reach the upstream as written, even when a thinking budget the relay
  derives from `reasoning_effort` is larger (ADR-0025's spirit).
- **No change to native Messages.** A Messages request already names its `max_tokens`, as the
  dialect requires; one that names none reaches the vendor as written and is answered by it.
- **No limit invented for any other Upstream.** codex drops `max_output_tokens` before the
  request leaves, and grok, antigravity and configured endpoints are sent a limit only when
  the client set one; each upstream then applies its own.
- **No new config.** The ceiling is the model metadata pengepul already holds.

## Acceptance criteria

- AC-1: A Chat Completions request with no limit (or a `null` one) for an anthropic model
  reaches the upstream with `max_tokens` equal to that model's **Output ceiling** from the
  fetched catalog.
- AC-2: The same holds for a Responses request with no `max_output_tokens`.
- AC-3: A model the catalog does not list yet takes its ceiling from the curated table, so
  `claude-opus-5-5` is asked for 128000 before the first catalog fetch lands.
- AC-4: A model no metadata describes is asked for 64000.
- AC-5: A client-set `max_tokens`, `max_completion_tokens` or `max_output_tokens` reaches the
  upstream unchanged, and a native Messages request with no `max_tokens` reaches it with none.
- AC-6: A request whose `reasoning_effort` asks for thinking and that names no limit reaches
  the upstream with `max_tokens` above the thinking `budget_tokens`.

## Verification

```sh
cargo test --locked --test app output_ceiling
cargo test --locked --test translate
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo fmt --check
```

Manual, against a relay started from this tree on a spare port: send one Chat Completions
request with no `max_tokens` to `anthropic/claude-opus-5-5` asking for a long answer, and
read in the debug log that the upstream body carried `max_tokens: 128000` and that the reply
reported more than 8192 output tokens.
