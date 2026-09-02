# providers-console

## Goal

Replace the console's empty Models page with Providers: the registry of upstream endpoints,
the models each one actually offers, and the rates that price them. Core gains one thing
only — it writes the catalog it already discovers to a table, so the console can read it.

## Non-goals

These were examined and found fine. A diff that touches them fails.

- **Core does not learn the anthropic dialect.** ADR-0011 scopes generic providers to Chat
  Completions. An `anthropic_messages` row stays registerable and unserved; the page says
  so plainly and does not make it work.
- No change to how core routes, resolves, prices or fails over. The single core change is
  persisting a list it already fetches.
- The built-in providers are not editable. anthropic and codex have no `Provider` row and
  must not gain one: `Model.providerId` and `UsageEvent.provider` name them by string, and
  a row would make the registry disagree with ADR-0014 about what a configured endpoint is.
  They appear in the list, they can be priced, and nothing else about them is writable.
- No credential handling in the console. Adding an endpoint does not add its key; that
  stays `pengepul login --provider <id> --key <k>`, which is ADR-0013's asymmetry.
- The console never calls core. It has no Local API key and no admin credential, and is not
  given one here. It reads the table core writes.
- Re-pricing never moves a past `UsageEvent`. The rates are copied onto the row at write
  time and stay there; this is already true and must remain so.
- No new dependency.

## Design

From `docs/DESIGN.md`, whose tokens are the only source of colour, type, spacing and radius.

Two routes. `/providers` lists every provider — the two built-ins and one row per registry
entry — with its base URL, its dialect, and how many of its discovered models carry rates.
`/providers/<id>` lists that provider's discovered models and prices them.

A configured endpoint added here does not route until the relay restarts, because the
account managers are built at startup. The page states that where the endpoint is added,
not in a footnote.

## Acceptance criteria

- AC-1: `/models` no longer exists, `/providers` does, the rail says Providers, and `/`
  still redirects to `/keys`.
  - Verify: load every route; `/models` answers 404 and `app/models/` is gone.
- AC-2: The list carries all three kinds — anthropic, codex, and one row per `Provider`
  row — each showing base URL (or that it is built in), dialect, and a count of how many of
  its discovered models are priced.
- AC-3: A configured endpoint can be added, edited (base URL and dialect) and deleted.
  Deleting removes the registry row and nothing else: its `UsageEvent` rows and the rates
  copied onto them stay readable, and its `Model` rate rows survive so re-adding the
  endpoint does not silently unprice it.
  - Verify: delete an endpoint that has usage; assert the usage rows and their `costUsd`
    are unchanged and the Logs page still renders them.
- AC-4: A built-in has no edit, no delete and no dialect control, and no code path can
  create a `Provider` row for one.
  - Verify: assert `Provider` is empty of `anthropic`/`codex` after exercising every write
    the page offers.
- AC-5: The console cannot write a row core will refuse. The id must be non-empty, must not
  contain `/`, and must not be `anthropic`, `codex` or `claude`; the base URL must be
  non-empty. These are core's own rules from `validate_providers`, restated where the row is
  created, and a duplicate id is refused rather than overwriting.
- AC-6: An endpoint whose dialect is `anthropic_messages` is registerable, and both the list
  and its own page mark it as not served, naming the dialect. It is not an error state.
- AC-7: Core writes `DiscoveredModel` on each catalog refresh: one row per provider per
  model id, replacing that provider's previous set. A fetch that fails leaves the previous
  set intact rather than emptying it, because an upstream blip must not read as "this
  endpoint offers nothing".
  - Verify: a test driving a refresh with a failing fetch after a successful one, asserting
    the rows from the successful fetch survive — **once per loop**. `refresh_model_catalog`
    reaches `record_discovery` through two arms, one for the built-in providers and one for
    the configured ones, and a wipe planted in either failure path is the same defect.
    Covering the configured arm and reading the built-in one is exactly how this shipped
    with the built-in path unguarded; a planted wipe there survived the whole suite.
  - Not verified, and deliberately: `record_discovery` runs *after* the in-memory catalog
    write, so a contended database cannot delay the list `/v1/models` serves. That ordering
    has no observable contract to assert without a timing-dependent test, so it is held by
    the comment beside it rather than by a flaky one.
- AC-8: No `DiscoveredModel` row is written when no database is configured, matching every
  other metering behaviour.
- AC-9: `/providers/<id>` lists that provider's discovered models, each with its four rates
  or a clear unpriced state, and rates can be set and cleared. A model discovered but never
  priced is normal, not an error.
- AC-10: Setting a rate changes what the next request records and moves nothing already
  recorded.
  - Verify: record usage, set rates, record again; assert the first row's `costUsd` is
    unchanged and the second reflects the new rates.
- AC-11: No literal colour, size or radius outside `app/tokens.css`, on the same terms as
  `console-shell.md` AC-5. Report the computed contrast ratio for every pair this adds.
- AC-12: The relay-restart requirement is stated in the UI where an endpoint is added.

## Verification

```sh
cargo test --locked
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings

cd web
./node_modules/.bin/tsc --noEmit
pnpm test
./node_modules/.bin/next build
```

Driven against the dev server, not assumed:

```sh
curl -s localhost:3000/providers | grep -c 'shell-rail'
curl -s -o /dev/null -w '%{http_code}' localhost:3000/models   # 404
```
