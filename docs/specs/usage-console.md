# usage-console

## Goal

Replace the console's last placeholder with Usage: what was spent and how many tokens went
through, per day and per key, over a range the operator picks. The final section of the
four the console was drilled for, and the one the metering exists to surface.

## Non-goals

These were examined and found fine. A diff that touches them fails.

- **No core change at all.** Every number here is already recorded. This is queries and a
  page; `core/` is not opened.
- No schema change and no migration. `UsageEvent` carries the tokens, the cost and the
  rates that priced it, and is already indexed on `startedAt` and `[keyId, startedAt]`.
- No new dependency, and no charting library. The design system names `accent` as a chart
  fill; a bar is a width, and this needs nothing a stylesheet cannot do.
- No JavaScript for the page. Filters are a GET form, exactly as Logs does it, so the state
  lives in the URL and the page stays a server component.
- `KeySpend` is not the source. It is a month-granular running total that exists so budget
  enforcement stays cheap on the relay's hot path; a date range needs the events. Usage is
  not a hot path.
- Nothing here writes. No action, no mutation, no re-pricing — that is the Providers page.
- Budgets are shown where they already are, on Keys. This page reports what happened, not
  what is allowed.

## Design

From `docs/DESIGN.md`, whose tokens are the only source of colour, type, spacing and radius.

One page, three bands: the totals for the range, a day-by-day table, and the keys ranked by
spend. The filter bar is the Logs grammar — `from`, `to`, and a key select — so the console
has one way to narrow a view rather than two.

The daily bars are a width on a real table row, not a chart on top of one: the numbers are
the content and the bar is a fill behind them. That is accessible by construction, needs no
SVG and no script, and it is what `accent` is for — a fill that never carries text, at
1.85:1 against the track it sits on.

## Acceptance criteria

- AC-1: `/usage` renders inside the rail shell with the totals, the daily table and the top
  keys. `app/placeholder.tsx` has no importer left and is deleted with it.
- AC-2: The range defaults to the last 30 days and can be set with `from` and `to`. The
  filter state lives in the URL, so a filtered view is linkable, and changing a filter does
  not carry a stale page number — there is no paging here.
- AC-3: The range is inclusive of both ends in UTC, matching how Logs reads the same two
  fields.
  ~~A range whose end precedes its start yields an empty result.~~ **Contradicted itself and
  is corrected.** The same sentence also said "matching how Logs", and Logs treats an
  inverted range as *unset*. One rule, not two: any range this page will not serve — a bound
  it cannot parse, an inverted pair, or a span past AC-3a's ceiling — falls back to the
  default window, and the page redirects so the URL says what is on screen. An empty result
  would have been a third behaviour for the same input class.
- AC-3a: The range has a ceiling, and the page cannot be made to render an unbounded one.
  `from` and `to` are arbitrary calendar days out of a URL and the daily table has a row per
  day, so `?from=0001-01-01&to=9999-12-31` otherwise asks for three and a half million rows
  — which is not a slow page but a server gone for minutes, taking every other page with it.
  - Verify: assert the parsed range never exceeds the ceiling for a set of hostile params,
    and that `usageByDay` returns no more rows than the ceiling for each.
- AC-3b: The URL always describes what is rendered. When the requested params differ from
  what the page will serve, it redirects to the canonical URL rather than displaying one
  range under an address bar naming another.
  - Verify: assert the canonical URL round-trips, so a redirect cannot loop.
- AC-4: Totals cover the range: requests, input and output tokens, and cost. Unpriced
  requests count toward tokens and requests and contribute nothing to cost, and the page
  says how many there were rather than letting them silently deflate the total.
  - Verify: a seeded range with priced and unpriced events, asserting each total.
- AC-5: The daily table has one row per day **in the range**, including days with no
  traffic, so a gap reads as a gap rather than closing up.
  - Verify: seed a range with an empty day in the middle; assert the row count equals the
    day count and that the empty day is present with zeroes.
- AC-6: Filtering by key restricts every band — totals, days and the ranked list — not just
  one of them.
- AC-7: The keys are ranked by cost, and the ranking names the key and its owner. A revoked
  key still appears if it spent inside the range: the money was spent.
- AC-8: A range with no events renders an empty state, not a zero-height table or a division
  by zero in the bar widths.
- AC-9: Money is formatted as it is on Logs: four decimals, because a single cheap request
  costs less than a cent and rounding to `$0.00` would report it as free.
  Not "as on Keys", which was wrong when written: Keys shows two decimals with a `<$0.01`
  escape. Both are honest about sub-cent amounts and they are different answers to it. The
  divergence is recorded here rather than silently inherited; changing Keys is out of scope.
- AC-10: No literal colour, size or radius outside `app/tokens.css`, on the same terms as
  `console-shell.md` AC-5. Report the computed contrast ratio for every pair this adds, and
  confirm no bar carries text on accent.
- AC-11: The daily aggregate is one query, grouped in SQL rather than by walking every event
  in JavaScript. A month of a busy relay is tens of thousands of rows and the page must not
  load them to add them up.
  - Verify: attach a query logger to the client under test — not a second client, which
    observes nothing and makes the assertion vacuous — count the statements a report issues,
    and assert every `UsageEvent` read carries an aggregate. Match the table name
    quote-agnostically: Prisma emits backticks here, and a pattern expecting double quotes
    silently finds nothing.

## Verification

```sh
cd web
./node_modules/.bin/tsc --noEmit
pnpm test
./node_modules/.bin/next build
```

Driven against the dev server, not assumed:

```sh
curl -s localhost:3000/usage | grep -c 'shell-rail'
curl -s -o /dev/null -w '%{http_code}' 'localhost:3000/usage?from=2026-08-01&to=2026-08-29'
curl -s -o /dev/null -w '%{http_code}' 'localhost:3000/usage?to=2026-01-01&from=2026-12-01'  # inverted
```
