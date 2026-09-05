# consistent-panels

## Goal

Every rich panel speaks one language. Today a reader meets three header
forms and four row forms across seven surfaces; after this, one header
grammar and one row grammar, so a row means the same thing whichever
verb printed it.

The user's words: *"we need to apply the same shape style for all"*,
then *"consisten style"*.

## The language

**Header** — `<subject>` alone, or `<subject> ─ <qualifier>`. No colons.
The subject names the thing (`relay total`, `pool anthropic`, `service`,
`login commandcode`, `update`, `config`). A qualifier must add a fact the
rows do not carry: a count qualifies (`2 pools, 3 accounts`, `1 account,
1 available`); a **state never does**, because a state always has its own
row and the header would only repeat it, truncated and uncolored.

**Row** — `<label>  <value>`, labels left-aligned in a column fitted to
the panel's own labels. One row, one fact.

**Glyph** — `●` prefixes a value that reports a *state* (`server`,
`state`, an account's availability), never a plain fact. Green ok, amber
attention, red failed, exactly as today.

**List rows** — an account row (`email  ● available  903 ok  ██ 100%`)
and a model row (`claude-opus-5  147 ok  23.9M`) keep their fitted
columns: they are a table, not a fact. They sit under the panel's fact
rows, indented for models.

**Separator** — a panel with both list rows and a fact rollup divides
them with `├───┤`. Only `accounts` has both, so it is the one panel
built by hand rather than by `fact_panel`.

## Non-goals

- **Plain output does not change.** Piped/`NO_COLOR`/`TERM=dumb` stays
  byte-identical to v0.10.2 plus this branch's already-landed changes
  (`status`, `accounts`). Scripts keep parsing what they parse. This is
  the user's explicit choice.
- **No new verbs, flags, colors, or panel width.** 64 columns stands.
- **`service logs`, `config show`, `help`, and errors on stderr stay
  plain** in both styles, as today.
- **The rich/plain wording split for one state is deliberate.** An
  account with no future cooldown reads `unavailable` in plain and
  `unresponsive` in rich. `CONTEXT.md` puts `unavailable` on the Cooldown
  avoid-list and `cli-style-dashboard` prescribes `unresponsive`, but
  plain bytes are frozen by the non-goal above, so the two cannot be
  reconciled without breaking a script. Recorded here so a later reader
  does not "fix" one of them.
- **No re-litigating what a panel *says*.** Only how it is shaped:
  `status` keeps its pool lines, `accounts` keeps its per-model lines.

## Acceptance criteria

- AC-1: One header helper produces every rich header. Headers carry no
  colon: `pool anthropic ─ 1 account, 1 available`,
  `login commandcode`, `service`, `relay total ─ 2 pools, 3
  accounts`, `update`, `config`.
- AC-2: One row helper produces every rich fact row: `label` padded to
  the panel's label column, two spaces, value. The label column is
  fitted to the longest label in that panel, capped so the value column
  never collapses.
- AC-3: `status` rows become labelled facts: `config`, `url`, `server`
  (glyph + health), one row per pool labelled by pool name, then
  `requests`, `tokens`, `reasoning` (only when non-zero), `total`.
- AC-4: `service status` rows become labelled facts: `state` (glyph),
  `enabled`, `pid`, `memory`, `cpu`, `tasks`, and `uptime` for a running
  unit or `stopped` for a dead one — dropping today's ad-hoc two-space
  pairs. The header is `service` with no qualifier: the state is already
  a row.
- AC-5: `service` actions, `login`, and `update` use `state` as the
  label of their glyph row, and name their subject in a second row:
  `state ● restarted`; `state ● saved` + `account key-90445c90`;
  `state ● latest` + `version 0.10.2`.
- AC-6: `config path` and `config api-key` are fact rows (`path`,
  `api key`) under a `config` header, unchanged in content.
- AC-7: Account rows and model rows keep their table shape and their
  fitted columns; they are not converted into fact rows.
- AC-8: Every rich line is exactly 64 visible columns. Plain output is
  unchanged by this spec: the surfaces whose plain bytes are stable
  (`config path`, `config api-key`, `update --check`, `--version`) are
  verified byte-identical against the pre-change binary, and the plain
  assertions in the suite pass unedited. `status` and `accounts` carry
  live counters, so their guard is those pinned assertions rather than a
  byte diff.
- AC-9: The label column is computed per panel, not per row or per
  section, so values align down the whole box — including the `accounts`
  panel, whose per-account token rows and footer rollup share one column.
  A label longer than the cap clips. A **value** longer than the box
  clips too — it is operator-supplied (a `--config` path, a url, an
  install path) and the renderer cannot shrink it — but the clip is
  always marked with an ellipsis, because a silently cut path reads as a
  real path that does not exist. Where the renderer *does* control the
  width (counts, token figures), it degrades to a compact form instead of
  clipping: a truncated number is a lie.

## Revisions

- **AC-4's header qualifier withdrawn.** The first cut headed the panel
  `service ─ active`, repeating the `state ● active (running)` row and
  repeating it worse — without the `(running)` detail and without the
  glyph's color. The user caught it. The rule is now explicit in the
  language above: a qualifier must add a fact the rows do not carry.
- **AC-9 reached further than the first cut.** The `accounts` panel's
  per-account token rows and footer rollup were still built by `format!`
  with a single space, so one box held four different value columns
  (13, 11, 14, 10). They are `Fact`s now, sharing the panel's column.
- **`fact_row` clips its label, and `panel_row` marks its clips.**
  Neither did, despite comments claiming otherwise: a provider key of 16+
  characters pushed the value column out of line, and 26+ pushed the
  token figure past the box edge where `panel_row` cut it — uncolored and
  unmarked. Two landing judges found the two halves. The label clips at
  the cap; an over-long value still clips, but now with an ellipsis, so
  truncation is visible. `rich-everywhere` AC-9 had exempted the `config`
  and `url` lines from clipping entirely by keeping them outside a box;
  status-total-only AC-2 put them in one, so this spec owes the mark.

## Verification

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Manual: every verb under `script -qec` and piped, the piped side diffed
byte-for-byte against the pre-change binary.
