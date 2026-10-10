## Getting started with development

### Requirements
`django-rusty-templates` is written in Rust, so you'll need to install the [Rust toolchain](https://www.rust-lang.org/tools/install).

To run the `just` recipes, you will need to have [Just](https://github.com/casey/just) installed.

### Python dependencies
#### Option 1: Using uv

[uv](https://docs.astral.sh/uv/) is a fast Python package installer and resolver. Using it will significantly speed up dependency installation.

First, install uv:

```bash
$ curl -LsSf https://astral.sh/uv/install.sh | sh
```

Then sync the dependencies and activate the automatically created virtual env:

```bash
$ uv sync
$ source .venv/bin/activate
```

#### Option 2: Using standard Python tools

If you prefer not to use uv, you can set up your development environment with standard Python tools:

```bash
$ python -m venv .venv
$ source .venv/bin/activate
$ pip install --group dev
```

Note: The `[dev]` dependency group is defined in `pyproject.toml` and includes all necessary development dependencies.

### Final step
**Compile translations**: run `django-admin compilemessages` in `tests/` directory

Running `just bootstrap` does it for you.

## Running tests

### Python tests with pytest
To run the Python tests, build Django Rusty Templates in develop mode with maturin and then run pytest.
Each change in rust needs a new execution of maturin develop.
```bash
$ maturin develop
$ pytest
```

Running `just python-test` combines these two commands for you.

### Django template conformance

Build the extension with `uv run maturin develop`, then run a Django version locally:

```bash
uv run scripts/run_django_test_suite.py --django-version 5.2 --parsed-output /tmp/django-5.2.txt --json-output /tmp/django-5.2.json
uv run scripts/run_django_test_suite.py --django-version 6.0 --parsed-output /tmp/django-6.0.txt --json-output /tmp/django-6.0.json
uv run scripts/run_django_test_suite.py --django-version 6.1 --parsed-output /tmp/django-6.1.txt --json-output /tmp/django-6.1.json
uv run scripts/run_django_test_suite.py --django-version 6.2 --django-ref dab0a5c47f159eb58472110dcbc10bd4a9b01adb --parsed-output /tmp/django-6.2.txt --json-output /tmp/django-6.2.json
```

Django 6.2 is development support: its cache initially clones `main`, because `stable/6.2.x` does not yet exist. Pin `--django-ref` for reproducible results. The reference above is the audited checkout, not a claim of compatibility with future changes.

Django 6 requires Python 3.12+. Development dependencies and the Ubuntu/Python 3.13 sanity job use Django 5.2. Draft PRs run only that sanity job; ready PRs add the Python 3.11–3.15 platform matrix (including 3.14t/3.15t), Rust OS coverage, cargo-shear, and Django 5.2–6.2 comparisons. Python 3.15 uses Django 6.1; Django 5.2/6.0 combinations remain excluded (#59).

The runner clones each series into its own ignored `scripts/.django-{version}` checkout. It substitutes Engine and the backend at runtime without changing Django's assertions, fixtures, or source files. Existing caches retain their reference until `--django-ref stable/6.1.x` (or a tag/commit) is supplied. JSON output records the Django/Python versions, reference commit, skipped tests, and failure tracebacks. The pass percentage counts runnable test methods once, including methods with multiple failing subtests, and excludes skips. A failing suite returns a nonzero exit code.

Use `--django-checkout /path/to/django` for an existing pristine checkout; the runner leaves it unchanged and rejects tracked source/test edits. An optional `--django-ref` must match that checkout's HEAD. Add test labels to narrow a run, for example `template_tests.test_partials`. `--raw-output` saves the complete test log.

### Rust tests with cargo
You can also run the Rust tests:

```bash
$ cargo test --workspace
```

If you get an `ImportError` from python, you may need to set the `PYTHONPATH` environment variable:

```bash
export PYTHONPATH=/path/to/venv/lib/python3.x/site-packages
```

If you get a `ModuleNotFoundError("No module named 'tests.settings'")`, you can work around the issue by adding the project root directory to the `PYTHON_PATH` environment variable:
```bash
export PYTHONPATH="/path/to/django-rusty-templates:/path/to/venv/lib/python3.x/site-packages"
```

## Pre-commit hooks

You can optionally install [pre-commit](https://pre-commit.com/#installation) hooks to automatically run some validation checks when making a commit:

```shell
$ pip install pre-commit  # or: uv tool install pre-commit
$ pre-commit install
```

## Coverage

When submitting a PR we check coverage. You can check coverage locally by running a command from the `justfile`:

```bash
$ just python-coverage
$ just rust-coverage
$ just rust-coverage-browser
```


### Comprehensive CI controls

This restoration remains a draft PR until approved. Marking it ready for review runs the broader jobs; it does not merge the PR. Draft pushes run only `Ubuntu / Python 3.13 sanity`, preserving the existing check name. `ready_for_review` triggers the deferred matrices. Manual dispatch can run each workflow explicitly. Workflows do not duplicate PR runs on main pushes, and wheel builds remain tagged-release/manual only.

Python covers Ubuntu, macOS, and Windows with 3.11, 3.12, 3.13, 3.14, 3.14t, and exact final versions 3.15.0/3.15.0t (the Ubuntu/3.13 matrix entry is covered by sanity). Python 3.15 entries install Django 6.1 and use `uv run --no-sync` to preserve that version; all other Python matrix entries use Django 5.2. Build steps assert a final interpreter and the expected Django series. Python 3.10 is removed from CI, without changing the package minimum version. Rust coverage runs on all three operating systems with serial embedded-Python tests. Python coverage runs once on macOS/3.13. Coverage reports are retained as artifacts for seven days; **Codecov uploads remain disabled**.

Django comparisons run both the PR and its exact base commit against the same pristine Django checkout for each series. Results include JSON, complete logs, and method outcomes; both baseline and PR failures retain a failing job status. Automatic comments run in a separate job with write permission only for PRs from this repository. Fork PRs still produce artifacts without a comment job; no privileged `pull_request_target` or `workflow_run` path executes PR code.

CodSpeed stays gated until the private repository is connected to CodSpeed and the repository variable `ENABLE_CODSPEED=true` is configured. No CodSpeed or Codecov credentials are added by this PR. Rust and uv caches, per-workflow cancellation, draft gating, and job timeouts limit repeated work. Ruff, rustfmt, and Clippy stay in sanity rather than running again in separate jobs; cargo-shear has its own job.

The repository currently has no branch protection or rulesets. Before enabling comprehensive CI, configure required checks as appropriate; the sanity check name is preserved. Cross-platform jobs and private CodSpeed integration must be verified on GitHub when the draft is made ready. Local workflow linting does not prove those hosted environments pass.
