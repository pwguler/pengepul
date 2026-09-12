# 21. clap's answer is an outcome, so only a runtime failure is an error

Status: Accepted

## Context

`run_with_env` opened with `Args::try_parse_from(raw)?`. That `?` flattened every
`clap::Error` into an `anyhow::Error`, and `main` renders every `Err` with
`eprintln!("{error:#}")` before `exit(1)`. clap carries two unlike things in that
one type, and the flattening lost the difference:

- `--help` and `--version`, which clap routes to **stdout** with status **0**.
- A usage error, which clap routes to **stderr** with status **2**.

So `pengepul --version` wrote `pengepul 0.19.0` to stderr and exited 1.
`pengepul --help` wrote 562 bytes to stderr and exited 1, which a pipe never saw.
`V=$(pengepul --version)` was empty — on the surface `consistent-panels` AC-8
names as the one most likely to be parsed by a script.

It survived three releases because a test pinned it.
`version_prints_the_same_bytes_in_both_styles` read:

```rust
let error = run_with_env(&["--version"], ...).expect_err("--version leaves through clap");
```

The comment says it "leaves through clap's own exit path". It does not: `main`
intercepts the error before that path is reached, so clap's `exit_code()` and
`use_stderr()` — the two functions that encode the convention — never ran. The
plain bytes the test asserted were correct. The stream and the status beside them
were not, and an assertion on `Err` cannot see the difference. `--help` had no
test at all.

## Decision

`run_with_env` matches clap's parse result rather than propagating it. `Err(error)`
returns `Ok(RunOutcome { code: error.exit_code(), .. })`, with the rendered text on
stdout when `!error.use_stderr()` and on stderr otherwise. Both accessors read the
same private predicate inside clap, so the stream and the status cannot disagree,
and the CLI re-derives neither.

`run_with_env` returning `Err` now means one thing: the command ran and failed —
invalid config, an unreachable relay, a refused harness. `main` keeps rendering
those to stderr with status 1.

## Consequences

- The `Err` arm of `run_with_env` stops meaning "the arguments were wrong".
  `login --provider opencode` still returns `Err`, because that check is a free
  string validated inside the verb rather than a clap parse; `launch openclaw` is a
  clap `ValueEnum` rejection and becomes an outcome with status 2. The two are
  indistinguishable from the outside and are not the same thing.
- A usage error now exits 2 rather than 1, matching clap and the convention every
  POSIX CLI follows. A caller keying on 1 to mean "bad flag" was already wrong;
  nothing in the repo did.
- Three tests pin the contract, each mutation-proven to fail when the production
  code is broken: `version_prints_the_same_bytes_in_both_styles` asserts stdout,
  empty stderr and status 0; `help_prints_to_stdout_and_succeeds` is new and asserts
  the same for `--help`; `launch_refuses_a_harness_it_does_not_know` asserts stderr
  and status 2. Restoring the `?`, swapping the stream, or hardcoding status 1 each
  fail all three.
- `consistent-panels` AC-8 already treats `--version` as script-read, so no spec
  changes: the clause was right and the code was not.
- The general lesson is worth more than the fix. An assertion that a call *failed*
  is not evidence about how a non-failure presents itself, and a test comment
  naming a mechanism is not evidence that the mechanism is reached. Both were
  present here, in the one test written to protect this surface.
- The same blindness sat one level up and was fixed with it. The `run` and
  `run_style` helpers asserted only that `run_with_env` returned `Ok`, so a happy
  path exiting non-zero was invisible to all 118 call sites using them (measured:
  66 `run(` plus 52 `run_style(`, definitions excluded — a `grep` for `run(&[` finds
  99, missing the `run(argv, ...)` form, which is the mistake an earlier draft made).
  This change adds two of its own, so the same count reads 120 afterwards. They now
  assert status 0 through `run_ok`, and `run_err` reads both refusal shapes — an
  `Err` and a non-zero outcome — because a helper that knew only about `Err`
  would report a rejected flag as a success. Two sites that discarded the result
  with `let _ =` now assert the refusal they were relying on.
- Measured, not assumed: making a happy path exit 1 fails 95 CLI tests where it
  previously failed 62. The 33 that gained sensitivity are tests that never
  checked a status at all. None of the original 62 covered the clap path, which
  is why the bug outlived them.
