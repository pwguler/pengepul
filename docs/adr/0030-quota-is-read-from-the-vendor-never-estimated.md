# 30. Quota is read from the vendor, never estimated

Status: Accepted

The relay learned about a plan's limits only from a 429. A pool could not show how close an
account was to its cap, or when a window would roll, and the panels said so on purpose
(`docs/specs/cli-style-dashboard.md`: "it never implies a plan limit, which the domain does
not have"). That stance no longer holds: each of the three vendors now states the numbers
itself, per account, on the wire.

- Anthropic sends `anthropic-ratelimit-unified-{5h,7d}-{utilization,reset,status}` on every
  response, success or not, and answers `GET /api/oauth/usage` with the same two windows as
  a percent plus an ISO reset.
- CommandCode answers `/alpha/billing/credits` with a five-hour and a weekly window (spend
  against a dollar cap) and the remaining monthly credit pool, and
  `/alpha/billing/subscriptions` with the plan id and the billing period's end.
- OpenCode answers `GET /zen/go/v1/usage` with rolling, weekly and monthly percentages and
  their resets.

pengepul keeps one snapshot per account: `source`, `observedAt`, and one entry per window
with a percent, a reset instant and the vendor's status word. A response's headers are read
at the one seam every outcome passes through (`send_json`/`send_stream`), so a success and a
429 both count. A poll of the vendor's own usage route covers an account with no traffic,
gated on the host the configured base URL names, and skipped while a snapshot is younger
than `quota.poll-minutes` unless a window has already rolled. The snapshot is persisted to
`<auth-dir>/<provider>/quota.json` and printed by `pengepul accounts` and
`GET /admin/accounts`.

Three rules keep the display honest:

- **No number is not zero.** A window with no observation, or one whose reset instant has
  passed, prints `n/a`. A zero would read as headroom.
- **The vendor's words decide the cap.** `LIMIT` comes from a percent at 100 or a status of
  `rejected`/`exceeded`, never from arithmetic on our side.
- **Only the three providers, only their own routes.** A configured OpenAI-compatible
  endpoint is never assumed to have a usage API; its panel is unchanged.

CommandCode's monthly pool is the one denominator no route reports. It comes from the plan
id's grant (GOAT 70, Go 10, Provider 15, Max 150, Ultra 300, Teams Pro 40), and the live
account corroborates the table: 70 less the period's spend equals the remaining credits the
API reports, to within a tenth. An unknown plan id leaves the month window unmeasured.

The CLI's bars are monochrome by operator request: the percentage and `LIMIT` carry the
reading, and a colored bar would be the one place a piped line and a panel differed in
meaning rather than in painting. A consumer with its own approved design may color its own
bars; the Windows dashboard does.

The dashboard's other two numbers come from two report routes rather than from arithmetic in
the widget: `GET /admin/status` (build, uptime, address, per-pool availability, today's
requests) and `GET /admin/usage?days=N` (requests and tokens per local day over every pool,
the stored peak, and the all-time totals with the prompt's cache split). Both are pure
functions over the same snapshots `/admin/accounts` serves, so the dashboard and
`pengepul accounts` cannot disagree about a figure.

`/admin/accounts` also takes a `?days=N` window, defaulting to the store's retention. A
widget that draws thirty days asks for thirty: the difference between a 76 KB payload and a
50 KB one is a body that can stall on a VPN. The admin responses are gzip-compressed for the
same reason, and only they are: a compressed byte stream cannot be read chunk by chunk, and
the inference API next door is exactly that.

## Considered options

- **A separate `pengepul quota` verb.** Rejected: the brief was to extend `accounts`, and a
  new verb would need its own key, its own route and its own style switch for one panel's
  worth of rows.
- **Estimate quota from token counts.** Rejected: utilization is a vendor formula over
  weighted tokens and plan-specific caps, and a token sum that ignores both is a number
  nobody can defend.
- **Poll every account every minute.** Rejected: a poll is a request the vendor sees per
  account. Freshness comes from traffic first, and the poll fills the gaps at the operator's
  interval.

## Consequences

- The relay now calls three undocumented vendor routes. They can change; a parse that
  yields nothing leaves the previous snapshot in place rather than writing a blank.
- A window that just rolled re-polls on the next minute tick, so the panel's `n/a` lasts at
  most about a minute, and an endpoint that keeps failing is retried no sooner than every
  five.
- A disabled account is not polled: `refresh_account` refuses to write a disabled
  credential, and its access token expires within hours. It keeps its last snapshot, which
  reads `n/a` once the windows it names have rolled.
- `quota.poll` and `quota.poll-minutes` are the switch and the cadence.
