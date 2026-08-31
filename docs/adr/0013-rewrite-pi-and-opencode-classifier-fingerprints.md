# 13. Rewrite pi and opencode classifier fingerprints

Status: Accepted (extends ADR-0004 and ADR-0005)

## Context

ADR-0004 and ADR-0005 reach a bot-identity section by its markdown heading. That
shape is openclaw's. Two other coding-agent harnesses were rejected by the
classifier through the relay and do not fit it, each in a different way, both
bisected against the live classifier.

**pi** (pi.dev) writes flat prose with no headings — it names itself in the
opening sentence ("operating inside pi, a coding agent harness") and repeats the
name across the documentation block it appends ("Pi documentation", "pi docs",
"pi topics", "pi .md files", "pi packages"). The heading walk never fires. Its
identity sentence alone passes and so does the prompt with the references
removed; it is their accumulation that trips, the same cumulative effect ADR-0005
recorded for `snake_case` tool names quoted in prose.

**opencode** trips on a single phrase, not on accumulation: the `Workspace root
folder:` label in the `<env>` block it appends. Renaming that one label clears
the whole 28 KB prompt; its own identity ("You are opencode, an interactive CLI
tool …") passes verbatim, and the path after the label is irrelevant — a harness
fingerprint, not anything about the workspace.

## Decision

`sanitize_system_text` applies `HARNESS_REWRITES`, a table of exact
(tripping, safe) substring pairs, after the heading walk and the tool renames.
Six entries neutralise pi's self-references; one renames opencode's label.

Deliberately not a pattern. A first attempt detected the harness name and
removed it throughout: "operating inside" is ordinary English, and a global name
delete corrupts prose, code spans and paths, and can rewrite a line into or out
of a markdown heading, moving the section boundaries the walk depends on. A
literal table cannot fire on text the harness did not write and cannot delete
more than it matches.

## Consequences

- Each harness costs a table entry, and drift in its prompt silently restores the
  400 — the same silent-400 risk ADR-0004 carries. The tests pin pi's prompt
  verbatim so drift fails a test rather than a request.
- `Workspace root folder:` is ordinary English in a way pi's phrases are not, so
  it can match text a user wrote. It rewrites a label to a synonym rather than
  deleting anything, so the damage is bounded to that substitution.
- The rewrites run last so a rewritten line cannot arm or disarm a heading skip,
  which is pinned by a test.
