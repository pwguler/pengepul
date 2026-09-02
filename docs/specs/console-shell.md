# console-shell

## Goal

Replace the console's centred document layout with a console shell: a forest-ink rail on
the left carrying the wordmark and navigation, full-bleed content to its right, and a thin
title bar holding the page title and its primary action.

## Non-goals

These were all examined and found fine. A diff that touches them fails.

- The key table keeps its columns, order and content. No new emphasis, no reordering, no
  responsive column hiding.
- ~~The create flow is unchanged.~~ **Reversed after seeing it built.** The card is gone
  and the title-bar action opens a modal. The one-time secret now lives in that modal, so
  Escape and the backdrop are blocked while it is on screen: the key exists nowhere else
  and must not be dismissed by reflex.
- No overview or dashboard page. `/` keeps redirecting to `/keys`.
- No behaviour change to what already existed: no key generation, hashing, revocation or
  server-action edits, and no change to the queries behind `/keys`. A full key is still
  shown exactly once and never retrievable, which is the load-bearing one.
  ~~No new query or schema.~~ **Reversed by two later asks.** The Logs page needed
  `lib/logs.ts` and its two queries, and providers needed a `dialect` column. Both are
  additive: nothing existing was re-shaped.
- Nothing outside `web/` **in this spec's changes**. Core rides the same branch under
  `docs/specs/keyed-usage.md` and is gated there, not here.
- No new dependency.
  ~~And no JavaScript added for layout.~~ **Reversed after seeing it built.** A collapsible
  rail was asked for, and collapse is a stored per-viewer preference, which CSS alone
  cannot hold. It is two things and no more: an inline `<head>` script that copies
  `localStorage` onto `<html data-rail>` before first paint, and a toggle button that
  writes it back. The layout itself is still CSS reading that attribute — nothing measures,
  nothing animates in JavaScript, and with scripting off the rail is simply expanded.
- The rail carries no status, health or spend. The console has no credential for core's
  admin routes and will not be given one here.

## Design

From `docs/DESIGN.md`, whose tokens are the only source of colour, type, spacing and radius.

- **Rail**: 232px, `primary` ink ground with `background` cream text — the console's one
  use of the inverse surface. Wordmark in the display face at `h3`; nav items stacked, the
  current one marked by a translucent cream fill rather than the khaki accent, which cannot
  carry text or a state indicator at 2.12:1.
- **Title bar**: page title at `h3` on the left, primary action right, separated from the
  content by a `border` hairline. Depth comes from that hairline, never a shadow.
- **Content**: fills the width remaining after the rail, padded on the `xl` step. No
  centred measure — a table is not prose.
- **Under 900px**, the one viewport threshold the design system names: the rail becomes a
  horizontal ink bar across the top, nav in a row that wraps if it must. One media query,
  no JavaScript, no toggle, nothing that can get stuck open.

## Acceptance criteria

- AC-1: Every page renders inside the rail shell: `/keys`, `/usage`, `/providers`, `/logs`.
  The rail shows the mark, the wordmark and all four sections, and marks the current one.
  (`/models` when this was written; `docs/specs/providers-console.md` replaced it.)
  - Verify: load each route against the dev server and assert the rail markup is present
    and `aria-current="page"` lands on exactly one link.
- AC-2: The rail carries nothing but the wordmark and navigation. No status line, no
  counts, no call to core.
  - Verify: grep the shell for any fetch, and assert no admin credential is read.
- AC-3: The title bar shows the page title and, on `/keys`, a *New key* action that opens a
  modal carrying the create form. A created key's secret is shown inside that modal and
  cannot be dismissed by Escape while it is visible. (Not "or the backdrop": a
  `showModal()` dialog does not light-dismiss, so there is nothing there to stop.)
  Dismissing it must actually drop the secret. `useActionState` has no reset, so the
  attempt is a child component behind a `key`, remounted on dismiss — otherwise the state
  survives, the next press of *New key* re-opens the dialog still showing the previous
  key, and no second key can be created without a full reload.
  - Verify: the page carries a `<dialog>`; no `#create` section remains; the cancel guard
    is exercised by driving Escape with a secret on screen; and create, dismiss, then
    press *New key* again — the form must come back empty, not the old secret.
- AC-4: Below 900px the rail is a horizontal bar and content is full width; above it the
  rail is vertical and can be collapsed to icons.
  Two media queries govern it, not one, and they are exact complements — `(max-width:
  900px)` and `not all and (max-width: 900px)` — so no viewport falls between them and no
  viewport matches both. The second exists because collapse is a width affordance and a
  top bar has no width to give back: every collapsed rule lives inside it, so below 900px
  those rules do not exist and the nav cannot be left stuck shut.
  - Verify: assert the stylesheet's only two `@media` rules are that pair, and that
    `shell-nav` is the only `'use client'` file in the shell.
  - The collapsed state reported to assistive technology must match what CSS actually
    applies, not what storage holds: below the breakpoint the labels are on screen
    whatever the stored preference says. The toggle carries `aria-pressed`, not
    `aria-expanded` — collapsing hides labels visually and nothing else, so every nav item
    stays named and reachable, and announcing the navigation as collapsed would be false.
- AC-5: No literal colour, size or radius in any declaration outside `app/tokens.css`.
  Three things are exempt, and the check must skip them or it reports itself:
  - Comments.
  - The media condition, wherever it is written. `var()` is invalid inside one, so the
    breakpoint is literal in both `@media` rules and in the `matchMedia` string that has
    to agree with them.
  - `app/icon.svg`, which carries two hex values and cannot do otherwise: a favicon is
    fetched as a standalone document, so it inherits neither `currentColor` nor a custom
    property from the page. The file says so at length, and the values are copies of
    `--color-primary` and `--color-background`.
  - Verify: scan every `.css`/`.tsx`/`.ts` outside `tokens.css` and the generated Prisma
    directory, skipping comment lines and media-condition lines, for `#hex` or `Npx`.
- AC-6: The non-goals hold: the key table's columns are byte-identical, and
  `app/keys/actions.ts`, `lib/keys.ts` and `lib/prisma.ts` are unchanged.
  `app/keys/create-key-form.tsx` is gone and `lib/logs.ts` is new; both are reversals
  recorded in the Non-goals above, not oversights.
  - Verify: compare the `KeyTable` markup byte for byte against the commit before this
    work, and list every file under `lib/` and `app/keys/` that changed — the set must be
    exactly the two named reversals.
- AC-7: The throwaway mockups are gone: `app/mockup/` does not exist.
- AC-8: Contrast holds. Cream on ink is 10.71:1; the active nav mark is a cream tint on ink,
  not the khaki accent. Report the computed ratio for every pair the shell introduces.

## Verification

```sh
cd web
./node_modules/.bin/tsc --noEmit
./node_modules/.bin/next build
pnpm test
# drive it, do not assume:
./node_modules/.bin/next dev -p 3000 -H 127.0.0.1
curl -s localhost:3000/keys | grep -c 'shell-rail'
```

Exercised in a browser over the tunnel at both widths, above and below 900px.
