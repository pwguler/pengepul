# 12. Learn the cloaked CLI versions at runtime; config pins are floors

Status: Accepted

## Context

Cloaking presents a vendor CLI version in the `User-Agent` and the billing
block (claude) and in codex headers. The values were baked defaults overridden
by `cloaking.cli-version`, so keeping them plausible meant a release or a
config edit every few weeks, and a stale value is exactly what the Classifier
would notice. A vendor CLI auto-updates, so a plausible client runs the latest
release; an install-date model would describe a user who never updates.

## Decision

`serve` fetches the latest versions once a day off the request path: npm
`dist-tags.latest` for `@anthropic-ai/claude-code`, GitHub releases/latest for
`openai/codex` (`rust-v` prefix stripped). Results rest in
`<auth-dir>/cloaking-versions.json` so an offline restart keeps them. The
effective version is `max(configured, fetched, baked default)` under semver
ordering: `cloaking.cli-version` and `cloaking.codex.cli-version` are floors,
not fixed values, so a config that still pins an old release is redundant
rather than wrong. A configured value that is not `major.minor.patch` is used
verbatim as the operator's explicit choice. The server listens before the
first fetch; a failed fetch retries within the hour and changes nothing.

## Considered Options

- Pin wins verbatim: rejected; every existing config would silently freeze
  at the version it was written with, the opposite of the goal.
- Bump the default in CI and ship via `pengepul update`: rejected; stale
  between releases, and a release exists only to move a string.
- Derive from install date via a shipped version table: rejected; models the
  wrong user and the table needs maintenance forever.

## Consequences

- One outbound GET a day each to registry.npmjs.org and api.github.com from
  the relay host; neither is a vendor API host.
- `X-Stainless-Package-Version` stays hardcoded; there is no public source
  for the SDK version a given CLI bundles.
- Pinning below the published version is impossible by design. Holding a
  version back needs a non-semver string, which is deliberate friction.
