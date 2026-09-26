# 24. The panels name the load, not the bill

Status: Accepted

## Context

The panels printed the token counters as `in`, `cache` and `out`, where `in` was the uncached
input — the prompt token no cache read *or* wrote — and `cache` was both cache directions
summed. Read as English the labels say the opposite: `in` is the input, and `cache` is the
cache read. The two most prominent numbers in every block were therefore misread. On a live
pool, `in 3.8M` sat beside a prompt of 1.3B, and `cache 1.3B (100%)` was computed with 66.0M
of cache *writes* in its numerator, so the printed "hit rate" was 100% where the read share
of the prompt was 94.8%.

There was no vendor vocabulary to copy, because the vendors disagree on the one word at
issue: Anthropic's `input_tokens` is the prompt token no cache served, OpenAI's
`input_tokens` and `prompt_tokens` are the whole prompt, and both call it input. The nearest
complete precedent is a Harness panel (pi), which prints `Input` / `Cached` / `Uncached` /
`Output` for one session, where Input is the whole prompt, Uncached folds the cache write,
and a write is disclosed as an annotation: `Uncached: 141 (100 written to cache)`.

## Decision

A panel's token block prints four rows, in this order, at every scope it reports a subject
for — the relay block in `status`, and the pool, account and model blocks in `accounts`:

- **Input** — the whole prompt the upstream saw, `cached + uncached`.
- **Cached** — the cache read, carrying its share of Input.
- **Uncached** — every prompt token no cache served, so it folds the cache write in. The
  glossary defines the word this way, which is what lets the row print bare.
- **Output** — every token the model generated, `reasoning` following it as its own row.

The share on `Cached` is `read / input`, the hit rate. The cache write and its 1h share are
not printed by any command: they stay recorded, and readable from `GET /admin/accounts` and
`usage.json`. The `total` and `pool` fact rows are removed, because `input + output` is the
carried load and both are printed.

Considered and rejected:

- **Fold the write with an annotation**, pi's shape exactly: `uncached 69.9M (66.0M written
  to cache, 6.8M at 1h)`. Rejected because the annotation restates the row it sits on, and on
  the pool that motivated the vocabulary (`claude-fable-5-1`, 70% of its writes at 1h,
  `claude-opus-5` beside it) the parenthetical is 95% of the number it annotates. A row that
  needs no breakdown is precisely the row that was being misread, so adding one there trades
  one misreading for another.
- **Give the write its own row.** Rejected as the third option: five rows per subject at four
  scopes, for a figure that changes no decision the four rows do not already carry.
- **Keep `in` / `cache` and fix only the ratio.** Rejected because the ratio was the smaller
  half of the defect: `in` still reads as the input, and `cache` still means two directions.

## Consequences

- **What is recorded does not change.** `input_tokens` keeps its meaning — the prompt token
  no cache read or wrote — and so do the field names in `usage.json` and in the admin
  payload. The printed `Uncached` is therefore *wider* than the recorded `input` counter by
  the cache write, and the label `Input` collides with the `input` in `totalInputTokens`.
  Both divergences are recorded in `CONTEXT.md`; neither changes a byte on the wire.
- **The panel stops reporting what a cache write cost.** A pool paying the 2x 1h premium and
  a pool paying 1.25x for 5m print the same block. The figures stay in the admin payload, and
  the CLI is no longer a billing surface for them — a deliberate narrowing, taken because the
  relay prices nothing and a raw write total is one more token count.
- **`status` and `usage` can no longer be checked against each other.** ARCHITECTURE's "One
  word, one scope" leaned on both printing the all-time carried figure from one sum, so the
  two could not drift. `usage` still prints `all time`; `status` prints it nowhere, and its
  pool rows carry per-pool load instead.
- **ARCHITECTURE's restatement rule is amended.** "A row never restates another row" was why
  the panel printed parts and no totals. A labelled total above its own breakdown is not a
  restatement, and that carve-out is what admits `Input`.
- **Plain output's byte stability is given up on purpose.** It was frozen so scripts could
  parse it; it now prints the same four words on one line per subject. The old `in` / `cache`
  labels and the `1h write` suffix are gone.
- **The model block prints under `accounts --verbose` only** (accounts-verbose). The block's
  rows and order are unchanged; the model scope as a whole moved behind the flag, because at
  every model an account ever served it was most of the view.
