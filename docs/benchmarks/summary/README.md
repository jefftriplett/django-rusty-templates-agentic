# Benchmark summary history

Each JSON file here is one run of `scripts/benchmark_summary.py`, kept so later runs can be compared with earlier ones. The table shows the latest comparison.

All runs so far used the same machine (an Apple Silicon Mac), Python 3.15.0, Django 5.2.17 and a release build (`uv run maturin develop --release`). Times are µs per call, best of 7. They are synthetic template timings, not HTTP request timings; see [What to expect](../what-to-expect.md) for how to read them.

## Runs

| Run | Commit | Measured | Raw data |
|---|---|---|---|
| 2026.11.3 release | `518b6ff` (2026-11-07) | 2026-11-12 | [2026.11.3.json](2026.11.3.json) |
| `main` after #408 | `33f4eaa` (2026-11-12) | 2026-11-12 | [2026-11-12-33f4eaa.json](2026-11-12-33f4eaa.json) |

Both runs used the `benchmark_summary.py` from `33f4eaa`, so the scenarios are identical. Django's own timings differed by at most 8% between the two runs; the table shows Django's from the `33f4eaa` run.

## 2026.11.3 compared with `33f4eaa`

| Scenario | Django | Rusty 2026.11.3 | Rusty `33f4eaa` | Speedup | vs Django, 2026.11.3 | vs Django, `33f4eaa` |
|---|---:|---:|---:|---:|---:|---:|
| **Warm, cached loader** | | | | | | |
| tiny template | 5.2 | 1.8 | 0.9 | 2.0× | 2.7× | 5.8× |
| table with filters | 5,590.5 | 436.2 | 332.6 | 1.3× | 13.0× | 16.8× |
| inheritance and includes | 189.1 | 30.2 | 20.0 | 1.5× | 6.4× | 9.4× |
| model-style objects | 1,617.9 | 269.4 | 132.6 | 2.0× | 6.0× | 12.2× |
| tiny template, 30 context keys | 5.0 | 1.8 | 0.8 | 2.2× | 2.7× | 6.2× |
| tiny template, 100 context keys | 5.0 | 2.0 | 0.8 | 2.5× | 2.5× | 6.2× |
| tiny template, 500 context keys | 5.0 | 2.9 | 0.8 | 3.6× | 1.7× | 6.2× |
| urlize on email addresses | 2,706.8 | 267.9 | 238.9 | 1.1× | 10.1× | 11.3× |
| custom simple_tag per row | 324.1 | 186.6 | 44.5 | 4.2× | 1.7× | 7.3× |
| custom filter per row | 344.7 | 64.5 | 45.8 | 1.4× | 5.3× | 7.5× |
| mixed template | 1,925.7 | 534.6 | 253.8 | 2.1× | 3.6× | 7.6× |
| **Load and render, no cached loader** | | | | | | |
| table with filters | 5,879.8 | 448.4 | 357.4 | 1.3× | 12.9× | 16.4× |
| inheritance and includes | 420.5 | 96.8 | 83.9 | 1.2× | 4.3× | 5.0× |
| mixed template | 2,022.9 | 654.5 | 341.3 | 1.9× | 3.1× | 5.9× |
| **Compile only, no cached loader** | | | | | | |
| tiny template | 9.8 | 1.3 | 0.9 | 1.4× | 7.7× | 11.5× |
| table with filters | 114.5 | 5.8 | 4.8 | 1.2× | 19.7× | 23.9× |
| inheritance and includes | 51.4 | 3.6 | 3.2 | 1.1× | 14.8× | 15.9× |
| mixed template | 48.2 | 76.1 | 54.7 | 1.4× | 0.62× | 0.88× |
| **Warm, debug=True** | | | | | | |
| table with filters | 5,592.4 | 420.2 | 327.7 | 1.3× | 13.3× | 17.1× |
| mixed template | 1,918.9 | 532.3 | 252.9 | 2.1× | 3.6× | 7.6× |

Compiling the mixed template is the one scenario where Django is faster. Its custom tag isn't marked as an isolated compiler, so Django's parser compiles the whole template before Rust compiles the native regions. Marking the tag, or listing it in the `isolated_tags` engine option, avoids that.

## Adding a run

1. Build a release extension: `uv run maturin develop --release`.
2. Run `uv run python scripts/benchmark_summary.py --json-output docs/benchmarks/summary/<name>.json` on a quiet machine. Name the file after the release, or after the date and commit for an unreleased `main`.
3. Add the run to the table above and update the comparison.

To measure an older commit, build it in a separate worktree and run the current script with that worktree's interpreter, so both runs use the same scenarios.
