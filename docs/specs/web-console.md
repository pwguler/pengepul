# web-console

## Goal

The ops console: four pages — Usage, API Keys, Models and Providers, Logs — on Next.js and
Prisma behind a shared login, built on the project design system.

## Non-goals

- No employee self-service, no SSO, no per-user roles. One shared login, one audience.
- A full Local API key is shown once at creation and never retrievable again. No reveal
  action exists anywhere in the UI.
- The console never writes an upstream credential to the database. Adding a static key for a
  configured provider calls core's admin endpoint, which writes the same 0600 token file the
  CLI writes.
- No OAuth flow in the UI. Adding an anthropic or codex Account stays `pengepul login` over
  SSH, because the vendor pins the redirect URI to localhost.
- Nothing here touches core's relay path or its request handling.
- The console edits no operator setting. Every one of them lives in `config.yaml`, and a
  console that could break its own boot is a trap.
- No charting library is added if the design can be met with the existing stack.

## Acceptance criteria

- AC-1: Any page requested without a session redirects to the login; the login accepts the
  configured shared secret and nothing else. Failing closed is tested, not assumed.
- AC-2: Creating a key displays the full key exactly once, with a copy action, and no route
  or API can return it again. The list shows name, Owner, prefix, Budget and month-to-date
  Spend.
  - Verify: a test asserting the create response carries the key and the list response for
    the same key does not.
- AC-3: Revoking a key sets `revokedAt`, and with core running that key's next relay request
  is refused. Exercised end to end, not only at the database.
- AC-4: The Usage page renders daily tokens and cost across a chosen date range, filters by
  one or more keys, and ranks top keys by tokens and by cost. Each is driven from
  `UsageEvent` with no client-side aggregation of unbounded rows.
- AC-5: Models and Providers supports create, edit and delete for a Provider and for a Model
  with its four per-million rates. Saving a static key for a Provider calls core's admin
  endpoint; the secret never reaches Prisma.
  - Verify: a test asserting no column in the database holds the submitted secret.
- AC-6: The Logs page lists entries for a key with time, model, all token counts, duration,
  status, cost, and the truncated prompt and completion, and marks an entry whose text was
  cut.
- AC-7: Every colour, size and radius comes from the tokens in `docs/DESIGN.md`; no
  hardcoded hex or Tailwind palette colour appears in the source.
  - Verify: a grep for `#[0-9a-fA-F]{6}` and for raw palette classes returns nothing outside
    the token definitions.
- AC-8: The stack conventions hold: no uncommented `any`, no native `fetch` or direct
  `axios`, React Query owns server state, forms are React Hook Form with Zod, and no file
  exceeds 300 lines.
- AC-9: Pages are responsive at the project's breakpoints and keyboard navigable, and text
  meets the contrast ratios recorded in `docs/DESIGN.md`.
- AC-10: An empty database renders every page without error: no key, no usage, no model.
- AC-11: The console is optional. Core has no build, runtime or startup dependency on it,
  and a database-backed install stays fully manageable without it via the CLI key commands.
  - Verify: core builds and serves with `web/` absent from the tree entirely.

## Verification

```sh
cd web
pnpm lint
pnpm exec tsc --noEmit
pnpm vitest run
pnpm build
```

Driven through the running app, with core up and a seeded database, to prove AC-2, AC-3,
AC-5 and AC-6 against real screens rather than unit tests alone.
