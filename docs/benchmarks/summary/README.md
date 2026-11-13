# Benchmark summary history

Each JSON file here is one run of `scripts/benchmark_summary.py`, kept so later runs can be compared with earlier ones. The table shows the latest comparison.

All runs so far used the same machine (an Apple Silicon Mac), Python 3.15.0, Django 5.2.17 and a release build (`uv run maturin develop --release`). Times are µs per call, best of 7. They are synthetic template timings, not HTTP request timings; see [What to expect](../what-to-expect.md) for how to read them.

## Runs

| Run | Commit | Measured | Raw data |
|---|---|---|---|
| 2026.11.3 release | `518b6ff` (2026-11-07) | 2026-11-12 | [2026.11.3.json](2026.11.3.json) |
| `main` after #408 | `33f4eaa` (2026-11-12) | 2026-11-12 | [2026-11-12-33f4eaa.json](2026-11-12-33f4eaa.json) |
| `main` after #428 | `5c9d8aa` (2026-11-13) | 2026-11-13 | [2026-11-13-5c9d8aa.json](2026-11-13-5c9d8aa.json) |

Every run used the `benchmark_summary.py` of its day; the scenarios haven't changed between them. Django's own timings differ by up to 8% from run to run, so treat a Rusty difference of that size as noise. The table shows Django's from the latest run.

## Rusty across the runs

| Scenario | Django | Rusty 2026.11.3 | Rusty `33f4eaa` | Rusty `5c9d8aa` | Latest vs Django |
|---|---:|---:|---:|---:|---:|
| **Warm, cached loader** | | | | | |
| tiny template | 5.2 | 1.8 | 0.9 | 0.9 | 5.8× |
| table with filters | 5,705.6 | 436.2 | 332.6 | 347.1 | 16.4× |
| inheritance and includes | 186.3 | 30.2 | 20.0 | 18.8 | 9.9× |
| model-style objects | 1,624.6 | 269.4 | 132.6 | 137.8 | 11.8× |
| tiny template, 30 context keys | 5.2 | 1.8 | 0.8 | 0.8 | 6.3× |
| tiny template, 100 context keys | 5.2 | 2.0 | 0.8 | 0.8 | 6.3× |
| tiny template, 500 context keys | 5.1 | 2.9 | 0.8 | 0.8 | 6.2× |
| urlize on email addresses | 2,730.2 | 267.9 | 238.9 | 132.2 | 20.6× |
| custom simple_tag per row | 325.6 | 186.6 | 44.5 | 45.4 | 7.2× |
| custom filter per row | 342.1 | 64.5 | 45.8 | 47.9 | 7.1× |
| mixed template | 1,871.9 | 534.6 | 253.8 | 252.9 | 7.4× |
| **Load and render, no cached loader** | | | | | |
| table with filters | 5,646.8 | 448.4 | 357.4 | 345.4 | 16.4× |
| inheritance and includes | 403.4 | 96.8 | 83.9 | 77.9 | 5.2× |
| mixed template | 1,991.7 | 654.5 | 341.3 | 338.8 | 5.9× |
| **Compile only, no cached loader** | | | | | |
| tiny template | 9.3 | 1.3 | 0.9 | 0.8 | 11.4× |
| table with filters | 112.9 | 5.8 | 4.8 | 5.0 | 22.7× |
| inheritance and includes | 52.6 | 3.6 | 3.2 | 3.2 | 16.3× |
| mixed template | 47.5 | 76.1 | 54.7 | 51.2 | 0.93× |
| **Warm, debug=True** | | | | | |
| table with filters | 5,345.9 | 420.2 | 327.7 | 319.6 | 16.7× |
| mixed template | 1,848.6 | 532.3 | 252.9 | 246.2 | 7.5× |

Between `33f4eaa` and `5c9d8aa` the work was mostly on matching Django's behaviour (#420–#429). The one scenario that moved beyond noise is `urlize`, which no longer re-checks Django's settings on every call (#428). `{% with %}`, `{% block %}` and several filters also got faster (#423, #428), but these scenarios don't exercise them much.

Compiling the mixed template is the one scenario where Django is faster. Its custom tag isn't marked as an isolated compiler, so Django's parser compiles the whole template before Rust compiles the native regions. Marking the tag, or listing it in the `isolated_tags` engine option, avoids that.

## Adding a run

1. Build a release extension: `uv run maturin develop --release`.
2. Run `uv run python scripts/benchmark_summary.py --json-output docs/benchmarks/summary/<name>.json` on a quiet machine. Name the file after the release, or after the date and commit for an unreleased `main`.
3. Add the run to the table above and update the comparison.

To measure an older commit, build it in a separate worktree and run the current script with that worktree's interpreter, so both runs use the same scenarios.
