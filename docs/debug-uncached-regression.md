# DEBUG/uncached regression follow-up (#109)

The regression reported against 2026.10.6 is reduced by 2026.10.7 in our matched disk-loaded component fixture, and the original application independently reports that its development slowdown is gone. These are separate measurements: the fixture measures template work, while the application measures requests through its dev database.

## Matched release comparison

Published 2026.10.5, .6 and .7 wheels were installed in separate Python 3.13.15 virtual environments with the exact same Django 5.2.7, asgiref 3.12.1 and sqlparse 0.6.0 versions. The same benchmark script and fixture run against all three environments, with explicit cached/uncached filesystem loaders and independently selected DEBUG values. Timing runs are serialized; each result is the median of nine batches of 100 operations, with 10,000 operations for cached lookups. Every engine's rendered output is checked against Django before measurement.

The fixture inherits from a native base and mixes custom compiler nodes, simple tags, inclusion tags, includes and URL reversal inside a loop. Source reading, compilation, loading, first rendering, warm rendering, compile+render and load+render are recorded separately. First rendering compiles fresh child templates outside the timer; dependent templates retain the selected loader caching policy. It does not simulate a cold database or process.

| DEBUG | Cached | Phase | .5 / Django | .6 / Django | .7 / Django | .7 / .6 normalized |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| False | False | compile | 1.559 | 2.254 | 1.953 | 0.867 |
| False | False | first render | 1.005 | 1.563 | 0.767 | 0.491 |
| False | False | warm render | 0.980 | 0.836 | 0.715 | 0.855 |
| False | False | load and render | 1.089 | 0.738 | 0.923 | 1.250 |
| False | True | compile | 1.503 | 1.864 | 1.850 | 0.993 |
| False | True | first render | 1.104 | 0.794 | 0.812 | 1.022 |
| False | True | warm render | 0.994 | 0.763 | 0.795 | 1.041 |
| False | True | load and render | 1.047 | 0.805 | 0.805 | 1.001 |
| True | False | compile | 1.420 | 1.465 | 1.199 | 0.819 |
| True | False | first render | 0.988 | 1.045 | 0.887 | 0.849 |
| True | False | warm render | 1.030 | 1.071 | 0.854 | 0.797 |
| True | False | load and render | 1.050 | 1.001 | 1.004 | 1.003 |
| True | True | compile | 1.355 | 1.457 | 1.264 | 0.867 |
| True | True | first render | 1.124 | 1.112 | 1.053 | 0.947 |
| True | True | warm render | 1.082 | 1.060 | 0.932 | 0.879 |
| True | True | load and render | 1.086 | 1.120 | 1.067 | 0.953 |

DEBUG/uncached compilation improves by about 18%, first rendering by 15% and warm rendering by 20% relative to .6 after normalizing against Django. The combined load+render phase is effectively unchanged in this run; the fixture therefore supports the compilation/rendering improvements, but does not reproduce the application's combined request-level improvement consistently. Cached production load+render is also effectively unchanged.

The non-DEBUG uncached load+render ratio is inconclusive: its Django control changes from 1,912µs (.6) to 1,335µs (.7), while Rusty changes from 1,411µs to 1,232µs. Normalizing those uneven control measurements produces 1.250 despite a lower raw Rusty time. Preserve that observation rather than interpreting it as either a proven regression or a proven gain.

Raw data: [2026.10.5](benchmarks/debug-uncached/2026.10.5.json), [2026.10.6](benchmarks/debug-uncached/2026.10.6.json), [2026.10.7](benchmarks/debug-uncached/2026.10.7.json).

The JSON files include unchanged Django control timings, platform, package versions and a fixture hash. Ratios divide each Rusty time by its matched Django time; the final column divides .7's ratio by .6's ratio. Controls use the same Django wheel and configuration throughout, but wall-clock measurements can drift. These are observations from one machine, not universal performance guarantees.

## Original application's confirmation

The original [djangotv.com benchmark update](https://github.com/django-news/djangotv.com/pull/209#issuecomment-6124282662), published October 12, 2026, reports 1,161 passing tests and no failures on .7. Its pooled Rusty/uncached ratio against Django/cached changes from 1.421 (.5) to 1.482 (.6) to 1.100 (.7). The matched uncached comparison changes from 1.191 (.6) to 0.888 (.7), using the median of page medians. Django's measured cached-loader benefit remains approximately 1.23–1.24× across those releases.

Cached request rendering also retains its gains: that report's pooled Rusty/cached ratio improves from 1.015 (.6) to 0.994 (.7), and `/videos/` reaches 0.967. Those are the application's own results, not a request-level claim derived from our fixture. Its single development machine, Docker, dev database and in-process test client limit generalization; 0.994 overall is effectively parity.

## What prevents a repeat

PR #120 removed eager native-variable compilation for mixed DEBUG templates and kept native inheritance parents on their compiled path. `tests/test_debug_mixed_templates.py` checks both loader policies, output, exception identity, source metadata, context cleanup and uncached file freshness; `tests/test_native_mixed_inheritance.py` covers native ancestors across all four DEBUG/cache configurations.

The existing performance workflow compares the component fixture against each PR's base with unchanged Django controls and a 15% normalized regression threshold across all four configurations. This follow-up adds a real first-render measurement and records fixture identity and release provenance. CI artifacts retain the raw timing data. This synthetic gate supplements correctness tests; it cannot guarantee detection of every application workload regression.

## Reproduce

Create three isolated environments and install each release with the same Django version, then run from this checkout (one environment at a time):

```sh
uv venv /tmp/rusty-debug-5 --python 3.13
uv pip install --python /tmp/rusty-debug-5/bin/python django==5.2.7 django-rusty-templates-agentic==2026.10.5
/tmp/rusty-debug-5/bin/python scripts/benchmark_mixed_templates.py --number 100 --repeat 9 --json-output /tmp/rusty-debug-5.json
```

Repeat with environments/releases .6 and .7. Avoid simultaneous builds or other heavy workloads while measuring. Published wheels are used here; benchmarking a Rust source change requires rebuilding its release extension first.
