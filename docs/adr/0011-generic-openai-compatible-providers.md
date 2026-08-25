# 11. Reintroduce a static-key provider class as configurable OpenAI-compatible endpoints

Status: Accepted

## Context

ADR-0010 removed the opencode provider because the pool was a
Claude/ChatGPT-subscription relay and a static-key reseller sat outside that
story. The operator now wants the relay to serve any OpenAI-compatible endpoint
(groq, openrouter, deepseek, ...) through one generic code path. ADR-0001's
`{kind, id}` ProviderId split was designed for exactly this, and the removed
opencode code (~1,120 lines) is the degenerate case being generalized: key as
access token, refresh-less account policy, prefix routing.

## Decision

One `ProviderKind::Generic` variant serves every OpenAI-compatible endpoint;
each endpoint is a `providers:` config entry (`base-url`) whose name is its
provider id. Keys arrive via a revived `pengepul login --provider <id> --key`
and live as degenerate tokens under `auth-dir/<id>/`, so the existing
AccountManager (rotation, cooldown, failover, usage stats, admin surface)
applies unchanged. Routing is prefix-only (`groq/llama-3.3-70b`); bare ids
never route to generic endpoints, so two endpoints serving the same model id
stay unambiguous. Generic endpoints accept only Chat Completions inbound — the
common denominator every OpenAI-compatible client speaks — with no new
translators; Messages/Responses inbound and count_tokens answer 501. Failover
never crosses endpoints. Each endpoint's `/v1/models` is fetched on the
existing 15-minute loop and advertised prefixed. Upstream requests carry
exactly `Content-Type` + `Authorization: Bearer`; no cloaking (there is no
billing classifier to appease) and no per-endpoint extra headers.

## Considered Options

- Per-vendor enum variants (Groq, Mistral, ...): rejected by ADR-0001; a vendor
  is a base URL and a key, not a line of Rust.
- Fully dynamic config-driven behavior flags: rejected; loses exhaustive
  matching and turns every behavior match into a config lookup.
- Cross-endpoint fallback chains (9router-style tiering): rejected; breaks the
  glossary's failover rule and silently rebills a request on an endpoint the
  client did not name.

## Consequences

- ADR-0010's stated cost is paid: "re-adding a static-key provider later means
  reviving the login/key path and a refresh-less policy from scratch." The
  revival is generalized rather than opencode-shaped.
- ADR-0001 is amended to the three-variant enum; CONTEXT.md drops "exactly two
  Providers" and narrows Cloaking to the anthropic/codex upstreams.
- Static-key accounts never enter Reauth: they have no refresh token, so a
  rejected key only cools down and comes back to fail again until the operator
  replaces it.
