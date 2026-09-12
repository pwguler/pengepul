# body-limit-reaches-the-extractor

## Goal

Make `body-limit` the limit a request body is actually read under, so the relay stops
turning away bodies the operator has allowed at axum's 2 MiB default.

## Non-goals

- **No change to authentication or routing.** The limit is applied where the body is read,
  not to who may send one.
- **No new dependency.** The limit comes from axum's own `DefaultBodyLimit`; no
  `tower_http::limit` layer is added.
- **No change to the upstream or to the success path's output.** One client-visible answer
  does change, and AC-2 requires it: an over-limit body is turned away by the extractor's
  plain-text 413 rather than the handler's JSON `request body too large`, together with a
  config error at startup instead of a per-request 500. Nothing else on the wire moves, and
  no panel, header or CLI surface changes.
- **`enforce_body_limit` keeps its semantics**: 411 for a missing `Content-Length`, 400 for a
  non-numeric one, and the relay's own `request body too large` for a declared length over
  the limit. The extractor is added in front of it, not in place of it.
- **No per-request 500 for an unparseable `body-limit`.** That case becomes a startup
  refusal instead; the old behaviour is deliberately not preserved.

## Acceptance criteria

- AC-1: A configured `body-limit` is the number the body-reading extractor enforces, so a
  body above axum's 2 MiB default is read when the configured limit allows it.
- AC-2: The configured number binds the extractor exactly: a body of exactly `limit` is read,
  and `limit + 1` is turned away by the extractor rather than by the handler's check.
- AC-3: The limit applies on every mount a client can reach, including `/v1/v1/*`, which is
  where a client pointed at the documented `http://host:port/v1` base URL arrives.
- AC-4: An unparseable `body-limit` refuses config load, with an error naming the setting and
  the value, rather than being answered per request.
- AC-5: An empty `body-limit` means unlimited, as the README documents.
- AC-6: `BodyLimit` has no invalid arm; the state cannot be represented in the request path.
- AC-7: `cargo test --locked` passes, and the two checks are exercised on a non-default limit
  so that neither axum's default nor the handler's check can satisfy the tests.

## Verification

```
cargo test --locked
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo clippy --locked --all-targets --all-features -- -D warnings
```

AC-1, AC-2 and AC-3 are verified by mutation, not by the tests passing: each of these must
fail `the_extractor_enforces_the_configured_limit_on_every_mount` —

```
delete the DefaultBodyLimit layer                                  (AC-1, AC-3)
BodyLimit::Limited(_) => DefaultBodyLimit::disable()               (AC-2)
max(configured, 2 * 1024 * 1024)                                   (AC-2)
move the layer onto the /v1 nest only, leaving /v1/v1 unconstrained (AC-3)
```

AC-4 is verified by making `load_config` swallow the parse error, which must fail
`an_unparseable_body_limit_is_refused_at_load`. AC-5 and AC-6 are verified by grep and by
`body_limit_loads_as_the_number_it_spells`.

Live, against a relay built from this tree with `body-limit: 200mb`: a 2,097,153-byte body
must be read (not 413), where the shipped build answered 413.
