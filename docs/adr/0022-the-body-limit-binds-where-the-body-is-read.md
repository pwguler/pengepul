# 22. The body limit binds where the body is read

Status: Accepted

## Context

`body-limit` never reached axum's body buffering. The API routes take a `Bytes` extractor,
whose size limit is governed by `DefaultBodyLimit`, and no such layer existed anywhere in the
repository. The effective limit was therefore axum's 2 MiB default regardless of what the
config said, and the body was turned away inside the extractor, before any handler ran.

`enforce_body_limit` could not catch it: it is called from `parse_request`, inside the
handler, by which time the body has already been buffered — the bytes were already spent. So
the relay answered with axum's text rather than its own, and a `body-limit: 200mb` turned away
everything past 2 MiB.

Measured against the shipped binary, the boundary was exact:

```
2,097,152 bytes -> 400  {"error":{"message":"invalid JSON body"}}          (read)
2,097,153 bytes -> 413  Failed to buffer the request body: length limit exceeded
```

In production the operator reported 12 occurrences across three models and both wire dialects —
reported rather than measured here, since this repository holds no such log — always
the same shape: context grows, the first turn over 2 MiB is turned away, compaction shrinks the
context, the next turn succeeds. Every one of those turns was well inside the operator's
configured 200 MB.

## Decision

`extractor_body_limit` maps the configured `BodyLimit` onto `DefaultBodyLimit`, applied in
`create_app_with_upstream` next to the CORS layer, so the number the operator wrote is the
number the body is read under. `Limited(n)` becomes `max(n)`; `Unlimited` becomes `disable()`.

The parse moves to config load. `parse_body_limit` returns a `Result`, `Config.body_limit` is
a resolved `BodyLimit` rather than a `String`, and the `Invalid` arm is gone: an unparseable
`body-limit` now refuses startup with a message naming the setting and the value, instead of
answering a 500 once per request on an otherwise-serving relay. The invalid state is
unrepresentable rather than merely unreachable, which is why no arm for it survives.

## Consequences

- **Which layer answers is decided by ordering, not by the number.** Both layers are handed
  the same limit, so neither can apply a different one. But the extractor reads first, so an
  honest oversized request is turned away there, under axum's message. The handler's
  `request body too large` is therefore unreachable over HTTP, where a declared length that
  disagrees with the bytes sent is a framing error hyper rejects before either check. It is
  kept, and is not dead code: it is the fallback for a declared length over the limit, it is
  what a body between a small configured limit and 2 MiB hits if the layer is ever lost, and
  `app_enforces_configured_body_limit_and_invalid_json` exercises it in process. An earlier
  revision of this change claimed the two "agree by construction", which is false about the
  answer the client receives and was corrected.
- **An empty `body-limit` is now genuinely unlimited, and that is a new surface.** Before this
  change there was no layer at all, so 2 MiB applied even to a config that said unlimited —
  the setting was as broken in that direction as in the other. `disable()` means no `Limited`
  wrapper, and the extractor runs *before* `require_api_key`, so any client that can open a
  socket can make the relay allocate without presenting a credential, repeatedly and
  concurrently. Measured: a 20 MiB body against an empty limit was buffered and handed to the
  handler. There is no amplification — a huge declared `Content-Length` costs nothing on its
  own, since `Collected` pre-allocates from nothing — but the ceiling that made this cheap is
  gone for operators who write the empty value. The default stays `200mb`, so this is a
  deliberate choice by whoever writes it, not a default anyone falls into.
- **A bad `body-limit` now stops verbs that do not serve traffic.** `load_config` parses it, so
  `config show`, `config api-key` and `status` all exit 1 until the file is fixed. That is the
  intended trade: `config show` refusing to print a file it cannot parse is a worse experience
  than the per-request 500 but a better signal, and it is the only way the request path can be
  free of the invalid state. `config path` still works, since it needs no parse.
- **Two tests pin the boundary, because one cannot.** A bare 413 does not say which limit
  produced it: a fix taking `max(configured, 2 MiB)`, or one limiting only the first `/v1`
  nest, leaves `a_body_above_the_extractor_default_still_reaches_the_handler` green. Both were
  verified to keep the entire suite green before
  `the_extractor_enforces_the_configured_limit_on_every_mount` was added, which asserts that
  `limit` is read and `limit + 1` is turned away by something other than the handler's check, on
  all eight nested POST routes. `README.md` documents `http://host:port/v1` as the base URL, so an
  Anthropic client arrives at `/v1/v1/messages` and the doubled nest is not a curiosity.
- `README.md`'s `body-limit` comment said "checked against Content-Length", which described the
  single-check design and is the misunderstanding this change exists to remove.
