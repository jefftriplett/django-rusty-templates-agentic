# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

An experimental reimplementation of Django's template language (DTL) in Rust, exposed to Python via PyO3/maturin. Goals: 100% compatible rendered output, error messages at least as useful as Django's, and better performance.

## AI policy (read AI_POLICY.md)

The maintainers have a strict LLM policy. The parts that affect how you work here:
- Never add an LLM to `Co-authored-by:` or any attribution in commits/PRs.
- Don't work on issues labelled "good first issue"; they're reserved for new human contributors.
- Don't write PR descriptions or review comments on the user's behalf for posting upstream; the human must author and be able to defend every change.

## Commits and branches

Work on a branch, never `main`. `origin` is our private fork (`jefftriplett/django-rusty-templates-private`): push branches there and merge via PR. `upstream` is `LilyFirefly/django-rusty-templates`: only fetch from it, never push or open PRs there.

## Commands

Setup: `uv sync && source .venv/bin/activate`, then `just bootstrap` (compiles the test translations in `tests/locale`; needs gettext).

**Any Rust change needs `maturin develop` before the Python tests see it.**

```bash
maturin develop && pytest                 # or: just python-test [pytest args]
pytest tests/filters/test_cut.py          # one file
pytest tests/filters/test_cut.py -k "cut01 and rusty"  # one case, Rusty engine only
cargo test --workspace                    # Rust unit + proptest tests
cargo test -p dtl-lexer                   # lexer crate only
cargo clippy --all-targets --all-features # CI lint; pedantic + nursery are deny
cargo fmt --check
ruff check && ruff format --check
cargo bench -p dtl_benchmark
just python-coverage | just rust-coverage
```

If `cargo test` hits a Python `ImportError` or can't find `tests.settings`, set `PYTHONPATH` to the repo root plus the venv's site-packages (see CONTRIBUTING.md).

### Django conformance suite

`uv run scripts/run_django_test_suite.py --parsed-output out.txt` clones Django `stable/5.2.x` into `scripts/.django`. It applies `scripts/django_tests_use_rusty_templates.patch`, which monkeypatches `django.template.Engine` and `DjangoTemplates` in `runtests.py` to use this engine, then runs Django's `template_tests` and `template_loader`. The parsed output starts with the pass percentage and an OK/FAIL/ERROR summary, followed by one line per test. Pass test labels to narrow the run, e.g. `... template_tests.filter_tests.test_cut`. On PRs, CI runs this against both `main` and the branch and comments with the diff, so a change that moves the pass rate shows up there.

## Architecture

Workspace crates:
- **`dtl-lexer/`**: pure Rust, with no PyO3. `core.rs` splits template source into text/variable/tag/comment tokens; `tag/*.rs` sub-lex the contents of specific tags (`for`, `if`, `include`, `load`, …); `variable.rs` lexes `{{ var|filter:arg }}`. Every token carries an `at: (start, len)` span, which is what drives the miette-style error reports.
- **root crate (`src/`)**: the PyO3 extension module `django_rusty_templates`.
  - `parse.rs`: `Parser` turns lexer tokens into a `TokenTree` of nodes (`Tag`, `Filter`, `IfCondition`, `For`, `Include`, `SimpleTag`, …). Built-in tags and filters are dispatched by name with `match` here. Anything unknown falls back to the Python libraries loaded via `{% load %}`: custom `simple_tag`/`simple_block_tag`/filters become `SimpleTag`/`ExternalFilter` nodes that call back into Python.
  - `filters.rs`: filter structs and their arguments. `render/filters.rs` implements `ResolveFilter` for each one.
  - `render.rs` + `render/`: the `Render`/`Resolve`/`Evaluate` traits; `render/tags.rs` renders tags; `render/types.rs` holds `Context` (a scope stack plus an autoescape flag) and the `Content`/`ContentString` types that track safe vs. unsafe strings.
  - `template.rs`: the `#[pymodule]` with the `Engine` and `Template` pyclasses, plus loader wiring (`loaders.rs`: filesystem, app dirs, locmem, cached).
  - `error.rs`: parse and render errors rendered with miette so they point at the source span.
- **`python/django_rusty_templates/__init__.py`**: the `RustyTemplates` Django backend (`BaseEngine` subclass). It collects templatetag libraries and constructs the Rust `Engine`.
- **`dtl-benchmark/`**: codspeed/criterion benchmarks.

### Adding a built-in filter or tag

Usually this touches: lexing (in `dtl-lexer` if the tag has its own syntax), the name dispatch and node type in `parse.rs` (plus a struct in `filters.rs` for a filter), rendering in `render/filters.rs` or `render/tags.rs`, and a test module in `tests/filters/` or `tests/tags/`.

Gotchas:
- `src/lib.rs` shadows `todo!` so unimplemented paths raise a picklable `NotImplementedError` instead of panicking. Django's parallel test runner depends on this, so use `todo!` rather than `unimplemented!`/`panic!`.
- Clippy denies `unwrap_used`.

## Tests

`tests/settings.py` defines four engines: `rusty`, `django`, `rusty_nocache`, `django_nocache`. The fixtures in `tests/conftest.py` (`template_engine`, `assert_render`, `render_output`, `assert_parse_error`) run each test against **both** Rusty and Django, so expected output has to match real Django. Where error messages differ intentionally (Rusty's miette output vs. Django's), tests assert each engine separately, often with `inline_snapshot`. Many tests in `tests/filters/` and `tests/tags/` are ported from Django's own test suite; a docstring at the top links the upstream file.
