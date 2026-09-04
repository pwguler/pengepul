# rich-everywhere

## Goal

Every `pengepul` command renders the same 64-column panel language that
`status`/`accounts` already use: `login`, `service` (install/start/stop/
restart/uninstall/status), `update`, and `config` (path/api-key/show).
One visual language, decided once at the edge (TTY, `NO_COLOR`,
`TERM=dumb`), identical to the existing `Style`.

## Shape

- Action commands open a small panel with the command's subject as the
  header: `service`, `login: <provider>`, `update`, `config`. Inside:
  one row per fact, the outcome carried by a `●` glyph painted green
  (success), amber (attention: update available, service not enabled),
  or red (failure paths reachable without panicking).
- `service status` parses systemctl/launchctl output into structured
  rows — state (with glyph), enabled, since, pid, memory, cpu, tasks —
  the same shape on Linux and macOS. When the service is not installed,
  the panel says so with an amber glyph instead of erroring.
- `status` loses its plain three-line header (`config:`, `url:`,
  `server:`); those facts move into the relay total block at the bottom:
  `config <path>`, `url <url> — server <state>`, then the two totals.
- `config path` and `config api-key` wrap their single value in a
  `config` panel when rich; piped output stays the bare value so
  `$(pengepul config api-key)` keeps working.
- `service logs` and `config show` remain plain text passthroughs in
  both styles (verbatim, never clipped).
- `help` (clap-rendered) is out of scope.
- Errors stay plain on stderr (existing `bail!` paths unchanged).
- Piped output, `NO_COLOR` (even empty), or `TERM=dumb` produces the
  exact bytes of today's output for every command; only the TTY path
  changes. Plain `status`/`accounts` text keeps its current layout,
  except the status header lines move to the relay block in both styles.

## Acceptance criteria

- AC-1: `pengepul service restart` on a TTY prints a `service` panel
  (64 visible columns) containing `●` and `restarted`; piped, it prints
  `restarted service` exactly as today.
- AC-2: `pengepul login --provider <configured> --key <k>` on a TTY
  prints a `login: <provider>` panel containing the key label and
  `● saved`; piped, `saved <provider> account token for <label>`.
- AC-3: `pengepul service status` on a TTY prints a parsed panel with
  at least state, enabled, since, and pid rows, glyph green when
  active; on macOS the same rows come from launchctl. Piped, output
  remains the platform tool's text (today's behavior).
- AC-4: `pengepul service status` with no service installed prints an
  amber `not installed` panel on a TTY and errors with the existing
  message when piped.
- AC-5: `pengepul update` and `pengepul update --check` render an
  `update` panel on a TTY (green `updated`/`latest`, amber when a
  newer release exists); piped output is byte-identical to today.
- AC-6: `pengepul config path` / `config api-key` print a `config`
  panel on a TTY and the bare value when piped; `config show` prints
  YAML verbatim in both styles.
- AC-7: rich `status` shows no header lines before the first pool
  panel; the relay block carries `config <path>` and
  `url <url> — server <state>`; plain `status` carries the same facts
  inside the relay block instead of the old header.
- AC-8: for every changed command, piped output on this commit differs
  from v0.9.2 only where this spec says it may (status/accounts plain
  relay-block lines; everything else identical bytes).
- AC-9: all panels are pure functions of their inputs with `now` or
  status text passed in, reuse `top_rule`/`panel_row`/`Style`, and
  every panel line measures at most 64 visible columns.

## Non-goals

- No interactive prompts, spinners, or progress redraws: one-shot
  render, print, exit.
- No change to `serve`, `help`, log passthrough, YAML passthrough,
  or exit codes.
- No new palette: green/amber/red/dim/bold remain the whole set.
- No change to the admin API payload shape or to `Style::from_tty`.
- No JSON/machine output mode.

## Verification

```
cargo test
cargo test --test cli
cargo clippy --all-targets -- -D warnings
cargo fmt --check
script -qec "./target/debug/pengepul service restart" /dev/null | cat -v   # AC-1 rich
./target/debug/pengepul service restart                                    # AC-1 plain bytes
script -qec "./target/debug/pengepul status" /dev/null | tail -6           # AC-7
```
