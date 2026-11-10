# Message threads stay harmless

## Goal

Neither a caller's thread request nor a vendor answer about one can hurt a Pool: a request
that uses Anthropic message threads is answered where it arrives, a client that resends it
without threads is served normally, and a 4xx answer never cools an Account.

## Non-goals

- No reproduction of Claude Code's identity: `cch`, the billing-header fields, the beta set or
  the identifying headers stay as they are.
- No threading of the relay's own requests, and no pass-through of a caller's `thread`.
- No change to codex, grok or configured-endpoint traffic.
- No change to Usage counters, quota reading, or the panels.

## Acceptance criteria

- AC-1 A Messages request carrying `thread` is answered `400` with
  `error.details.error_code == "thread_unsupported_request"`, reaches no Account, and produces
  no Upstream call.
- AC-2 A caller that resends the same request without `thread` is served as any other request.
- AC-3 An Upstream 4xx other than 401, 403 and 429 is recorded as a refusal: no Cooldown, no
  failure streak, and the counters still balance against total requests.
- AC-4 A 401, 403 or 429 still costs the Account health and still fails over.
- AC-5 A threaded turn changes nothing in Usage accounting because no threaded request reaches
  an Upstream.

## Verification

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- Demo: `~/handoffs/pengepul-anthropic-cache/demo-isolated-env.sh <binary>`. It starts the
  relay on its own port with a synthetic Account and no network, and shows the thread request
  answered locally with zero Upstream calls, the same request without `thread` reaching the
  Upstream, and no Cooldown anywhere.

## Decisions so far

- [Approach]: compatibility only. A thread request is refused with the vendor's own code, which
  the SDK thread helper, the SDK tool runner and pi already fall back on (commit `8986eb88`).
- [Why not support threads]: a thread lives on the Account that created it, and the relay's
  Cloaking rebuilds `system` while the tool-name map comes from `tools`, so a continuation
  cannot cross the relay intact.
- [Why not imitate]: Claude Code 2.1.322 computes its billing `cch` in bundled native code that
  an outside client cannot reproduce without forging the vendor's integrity check.
- [4xx rule]: only 401, 403 and 429 are Account health; every other 4xx is the request's.
- [Testing]: isolated env only — own port, own config and auth dir, synthetic Account, no
  network. The running service is never used.
