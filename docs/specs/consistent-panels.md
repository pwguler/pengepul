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
`login commandcode`, `update`, `config`); the qualifier counts or states
it (`2 pools, 3 accounts`, `1 account, 1 available`, `active`).

**Row** — `<label>  <value>`, labels left-aligned in a column fitted to
the panel's own labels. One row, one fact.

**Glyph** — `●` prefixes a value that reports a *state* (`server`,
`state`, an account's availability), never a plain fact. Green ok, amber
attention, red failed, exactly as today.

**List rows** — an account row (`email  ● available  903 ok  ██ 100%`)
and a model row (`claude-opus-5  147 ok  23.9M`) keep their fitted
columns: they are a table, not a fact. They sit under the panel's fact
rows, indented for models.

## Non-goals

- **Plain output does not change.** Piped/`NO_COLOR`/`TERM=dumb` stays
  byte-identical to v0.10.2 plus this branch's already-landed changes
  (`status`, `accounts`). Scripts keep parsing what they parse. This is
  the user's explicit choice.
- **No new verbs, flags, colors, or panel width.** 64 columns stands.
- **`service logs`, `config show`, `help`, and errors on stderr stay
  plain** in both styles, as today.
- **No re-litigating what a panel *says*.** Only how it is shaped:
  `status` keeps its pool lines, `accounts` keeps its per-model lines.

## Acceptance criteria

- AC-1: One header helper produces every rich header. Headers carry no
  colon: `pool anthropic ─ 1 account, 1 available`,
  `login commandcode`, `service ─ active`, `relay total ─ 2 pools, 3
  accounts`, `update`, `config`.
- AC-2: One row helper produces every rich fact row: `label` padded to
  the panel's label column, two spaces, value. The label column is
  fitted to the longest label in that panel, capped so the value column
  never collapses.
- AC-3: `status` rows become labelled facts: `config`, `url`, `server`
  (glyph + health), one row per pool labelled by pool name, then
  `requests`, `tokens`, `reasoning` (only when non-zero), `total`.
- AC-4: `service status` rows become labelled facts: `state` (glyph),
  `enabled`, `pid`, `memory`, `cpu`, `tasks`, `uptime` — dropping
  today's ad-hoc two-space pairs. The header carries the state as its
  qualifier.
- AC-5: `service` actions, `login`, and `update` use `state` as the
  label of their glyph row, and name their subject in a second row:
  `state ● restarted`; `state ● saved` + `account key-90445c90`;
  `state ● latest` + `version 0.10.2`.
- AC-6: `config path` and `config api-key` are fact rows (`path`,
  `api key`) under a `config` header, unchanged in content.
- AC-7: Account rows and model rows keep their table shape and their
  fitted columns; they are not converted into fact rows.
- AC-8: Every rich line is exactly 64 visible columns; every plain
  output is byte-identical to before this spec.
- AC-9: The label column is computed per panel, not per row or per
  section, so values align down the whole box.

## Verification

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Manual: every verb under `script -qec` and piped, the piped side diffed
byte-for-byte against the pre-change binary.
