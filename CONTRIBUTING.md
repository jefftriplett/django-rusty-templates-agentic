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
```

Django 6 requires Python 3.12+. Development dependencies and the single Ubuntu/Python 3.13 CI sanity job continue to use Django 5.2; additional versions run locally without expanding CI.

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
