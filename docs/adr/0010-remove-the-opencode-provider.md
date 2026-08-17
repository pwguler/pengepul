# 10. Remove the opencode provider

Status: Accepted

## Context

pengepul relayed three provider families: anthropic, codex and opencode. The
opencode provider was a static-key, OpenAI-compatible reseller gateway
(`opencode.ai/zen/go/v1` and `/zen/v1`) with its own login path (a `--key` flag
or an import from opencode's `auth.json`), its own model-catalog arms (go and
credits lists), its own routing prefix (`opencode/<id>`, the only prefixed
provider), and a refresh-less account policy. The operator decided to drop it:
the pool is a Claude/ChatGPT-subscription relay, opencode was a key reseller
outside that story, and every opencode code path carried ongoing surface area
for a provider nobody used.

## Decision

Remove opencode support entirely. `ProviderKind::Opencode` and
`ProviderId::opencode()` are gone; the account manager, OAuth-adjacent login
path, model-catalog arms, routing/streaming transforms and the
`opencode_headers`/base-URL constants are deleted. The `--key` login flag
(which existed only for opencode) and `RefreshPolicyKind::Never` (which existed
only for opencode's static keys) are deleted with it. `src/providers.rs` held
nothing but the opencode prefix helpers and is deleted.

Leftover opencode credentials on disk are deliberately ignored, not purged and
not migrated: the binary never reads, advertises, or deletes them. The operator
removes `auth-dir/opencode/` and legacy flat `opencode-*.json` files by hand if
they want them gone. Stale requests need no special handling: `opencode/...`
model ids hit the generic 400 `unknown model` path (the routing prefix
heuristic lost its opencode arm) and `--provider opencode` dies at argument
parsing. No opencode-specific rejection code exists anywhere.

## Consequences

- Routing is now purely anthropic/codex; the explicit-prefix mechanism remains
  for those two, and an id nobody claims is rejected with 400.
- ADR-0001 is amended to the two-variant enum. The superseded record of the
  old opencode-prefixed routing (previously ADR-0003) is retired with the
  code it described, so the tree carries no reference to the removed provider.
- A client still sending `opencode/...` gets a loud, honest 400 rather than a
  silent misroute — the same failure a genuinely unknown model gets.
- Re-adding a static-key provider later means reviving the login/key path and
  a refresh-less policy from scratch, which is the intended cost of removal.
