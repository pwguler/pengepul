# version-uptime-and-usage-total

## Goal

Two gaps in the operator's two summary verbs:

- `pengepul status` says where the relay is and whether it answered, but not **which build
  is running** or **how long it has been up**. The operator's words: *"i need in pengepul
  status you show the uptime and version of the service"*.
- `pengepul usage` shows the 30-day trend and no relay-wide numbers. The operator's words:
  *"and in usage show this as well requests 16,563 (16,200 ok, 363 failed) input 4.0B …"* —
  the aggregate block `status` already prints.

## Shape

Both sources are local. `status` is a client and cannot learn the running process's version
from the relay, so it prints the version of the binary making the call, and it says so when
that binary is newer than the service that is running.

- **version** — `CARGO_PKG_VERSION`, the installed build.
- **uptime** — the same value `pengepul service status` shows, from the same parse: the
  `active (running) since …; 4min 28s ago` line of `CliRuntime::service_status()`
  (`src/service.rs`, `parse_relative_seconds`). One code path, so the two verbs cannot
  disagree.
- **staleness** — the service start instant is `now − uptime`, so when the binary's own
  mtime is later than that instant, the version row is marked and says to restart. This
  catches the case that motivated the ask: `pengepul update` without `pengepul service
  restart`. It cannot catch a running process whose file has since been replaced by an older
  one, and the spec records that limit rather than pretending otherwise.

`usage` gains the relay total **aggregate** under the trend: the `relay total: P pools, A
accounts` header and the counters (`requests N (S ok, F failed)`, input, cached, uncached,
output, reasoning). It does not gain `config`, `url`, `server` or the per-pool lines: those
are where-and-which facts, `status` is their home, and `accounts` owns per-pool detail.
The numbers are the relay's lifetime totals, the same figures `status` prints — not the
30-day window the trend above them covers, which is why they keep the `relay total` header.

## Non-goals

- **No relay-side change.** `/health` stays `{"status":"ok"}` and the admin payload keeps
  its shape; both new facts are computed locally, so nothing about the running process's
  build is exposed on an unauthenticated route.
- **No per-pool lines in `usage`**, and no `config`/`url`/`server` there either (see Shape).
- **No new flag.** No `status --json`, no `usage --total`.
- **No change to `accounts` or `service`.** `service` already prints uptime; this spec does
  not add version there.
- **No version or uptime in the panels'** pool rows, account rows or per-model tables.
- **The trend is untouched.** `usage`'s existing lines keep their bytes; the block is
  appended below them.
- **No staleness guess from the payload.** The relay is not asked anything new, so a
  mismatch can only be inferred from local files and the service clock.

## Acceptance criteria

- AC-1: `status` (plain) prints `version <v>` and `uptime <d>` after the `url … — server …`
  line and before the first pool line. `<v>` is `CARGO_PKG_VERSION`; `<d>` is the same
  duration string `service status` prints for the same systemd text.
- AC-2: `status` (rich) renders the same two as labelled rows named `version` and `uptime`,
  in the same position, in the existing box; every line stays at or under 64 visible columns
  and the borders are exactly 64.
- AC-3: When the service manager cannot answer — unit absent, `systemctl` missing, non-zero
  exit — the `uptime` row is omitted and `status` still succeeds with its other lines. The
  same holds for the staleness marker. Observability is never a gate.
- AC-4: When the binary's mtime is later than `now − uptime`, the version row carries the
  staleness marker: rich uses an attention glyph, plain appends a restart note. When the
  binary is older or the clock and mtime cannot be read, the row is unmarked.
- AC-5: `usage` (plain) prints the trend lines unchanged, then a blank line, then
  `relay total: P pools, A accounts` and the aggregate lines — byte-identical to the
  corresponding lines of `status` for the same payload.
- AC-6: `usage` (rich) prints its existing trend panel unchanged, then the relay total as a
  second box whose header is `relay total ─ P pools, A accounts`, with the aggregate rows
  and no pool rows. Every line is at most 64 visible columns.
- AC-7: The totals are the relay lifetime sums over every account of every pool, including
  pools with zero accounts in `P` — the same arithmetic `status` does, taken from the same
  function rather than recomputed.
- AC-8: An empty relay prints the block with `0 pools, 0 accounts` and zeroed counters in
  both verbs.
- AC-9: The renderers stay pure: `now`, the binary's mtime and the service text are read at
  the CLI edge and passed in, as `status-total-only` AC-8 and `usage-trend` AC-10 require.
  Purity is what makes AC-1/AC-4/AC-5 testable from a fixture.
- AC-10: `accounts` output is byte-identical to before, and the existing `status` and
  `usage` assertions pass unedited apart from the two inserted lines and the appended block.

## Verification

```sh
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings

./target/debug/pengepul status                 # version + uptime rows, block below
./target/debug/pengepul usage                  # trend, then the relay total
script -qec "./target/debug/pengepul status" /dev/null   # rich panel intact at 64 columns
script -qec "./target/debug/pengepul usage"  /dev/null
```

AC-3 and AC-4 are driven through `FakeRuntime`'s `service_status_error` and a fixture mtime,
so no systemd is needed to test the degraded and stale paths.
