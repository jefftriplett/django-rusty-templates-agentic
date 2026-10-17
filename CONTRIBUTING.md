## Getting started with development

### Requirements
`django-rusty-templates-agentic` is written in Rust, so you'll need to install the [Rust toolchain](https://www.rust-lang.org/tools/install).

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

## Changelog and releases

After merging a PR, update `CHANGELOG.md` under **Unreleased** with a short, human-readable summary and links to the merged PRs. Keep each release summary to one sentence, or two at most.

Before building release wheels or publishing, move the Unreleased changes into a section for the new version and include that update in the release PR. Check that every user-visible change since the previous release is covered, merge the release PR, then tag and publish with `just release`.

## Running tests

### Python tests with pytest
To run the Python tests, build Django Rusty Templates (Agentic) in develop mode with maturin and then run pytest.
Each change in rust needs a new execution of maturin develop.
```bash
$ maturin develop
$ pytest
```

Running `just python-test` combines these two commands for you.

### Django template and admin conformance

Build the extension with `uv run maturin develop`, then run a Django version locally:

```bash
uv run scripts/run_django_test_suite.py --django-version 5.2 --parsed-output /tmp/django-5.2.txt --json-output /tmp/django-5.2.json
uv run scripts/run_django_test_suite.py --django-version 6.0 --parsed-output /tmp/django-6.0.txt --json-output /tmp/django-6.0.json
uv run scripts/run_django_test_suite.py --django-version 6.1 --parsed-output /tmp/django-6.1.txt --json-output /tmp/django-6.1.json
uv run scripts/run_django_test_suite.py --django-version 6.2 --django-ref dab0a5c47f159eb58472110dcbc10bd4a9b01adb --parsed-output /tmp/django-6.2.txt --json-output /tmp/django-6.2.json
```

Django 6.2 is development support: its cache initially clones `main`, because `stable/6.2.x` does not yet exist. Pin `--django-ref` for reproducible results. The reference above is the audited checkout, not a claim of compatibility with future changes.

Django 6 requires Python 3.12+. Development dependencies and the default Ubuntu/Python 3.13 sanity job use Django 5.2. Every PR also checks unchanged upstream Django 5.2, 6.0, and 6.1 template and admin view tests on Python 3.13. Broader OS/Python matrices and Django 6.2 run by manual dispatch.

The runner clones each series into its own ignored `scripts/.django-{version}` checkout. By default it runs `template_tests`, `template_loader`, and `admin_views`, including login, index, change forms, filters, and inclusion tags through the configured backend. It substitutes Engine and the backend and adapts the public `Template(...)` constructor to compile and render through Rusty, without changing Django's assertions, fixtures, or source files. The base Django Template remains available for interoperability and test-client instrumentation. This matters because replacing Engine alone lets direct `Template(...)` tests pass using Django's parser and renderer.

Existing caches retain their reference until `--django-ref stable/6.1.x` (or a tag/commit) is supplied. JSON output records the Django/Python versions, reference commit, test labels, skipped tests, and failure tracebacks. The pass percentage counts runnable test methods once, including methods with multiple failing subtests, and excludes skips. A failing suite returns a nonzero exit code.

On Django 6.0+, the default run also includes CSP context processor, middleware, decorator, and utility tests. Local regression tests cover every registered builtin tag and filter in template syntax, since many upstream filter tests only call the Python functions. CSP integration tests additionally check the documented inline nonce, attribute tag, and media forms against enforced and report-only headers, fresh nonces per request, and lazy generation.

Use `--django-checkout /path/to/django` for an existing pristine checkout; the runner leaves it unchanged and rejects tracked source/test edits. An optional `--django-ref` must match that checkout's HEAD. Add test labels to narrow a run, for example `template_tests.syntax_tests.test_csp_nonce_attr` on Django 6.1, or `admin_views.tests.AdminViewPermissionsTest.test_login`. `--raw-output` saves the complete test log. Browser tests remain skipped unless their dependencies and browser configuration are supplied.

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


### CI controls

Every PR runs the Ubuntu/Python 3.13 sanity job (local Python and serial Rust tests, Ruff, rustfmt, and Clippy), plus upstream template conformance against Django 5.2, 6.0, and 6.1. Same-repository, non-draft PRs receive a summary comment. These four test jobs protect stable-version compatibility without running the full OS matrix.

Once this PR is merged and the workflow is present on the default branch, open **Actions → Full CI → Run workflow** and select the branch for comprehensive coverage. This manually calls the Python, Rust, lint, and Django workflows from that same revision. After these workflow definitions exist on the default branch, the CLI equivalent is:

```bash
gh workflow run full.yml --ref YOUR_BRANCH --repo jefftriplett/django-rusty-templates-agentic
```

Individual workflows remain dispatchable. `Full CI` runs the Python 3.11–3.14/3.14t and exact final 3.15.0/3.15.0t matrix across Ubuntu, macOS, and Windows, serial Rust OS coverage, cargo-shear, and Django 5.2–6.2 conformance. Python 3.15 entries use uv-managed final runtimes and Django 6.1 with `--no-sync`; Django 5.2/6.0 are excluded on 3.15. The package requires Python 3.11 or newer. Performance comparisons run locally on a controlled machine, never on GitHub-hosted runners.

Coverage reports are retained as artifacts for seven days. **Codecov uploads remain disabled.** Wheel builds remain tag/manual only. No duplicate main-push test runs or schedules are added. The restoration PR remains unmerged until approved.

Django base/PR comparisons use the exact same pristine upstream checkout. Structured summaries reject mismatched references and empty results; suite failures retain failing job status. Manual runs use the selected commit for both sides, and report results without posting a PR comment. PR comment permissions are confined to a separate job for same-repository PRs.

uv cache keys include job/matrix-specific suffixes so concurrent jobs do not compete to save the same key. Rust caches, timeouts, and workflow-specific cancellation groups limit repeated work. Cancellation groups use explicit workflow prefixes so reusable workflows called by `Full CI` cannot cancel each other or their caller. Linux runners are pinned to `ubuntu-24.04` to avoid the October–November 2026 `ubuntu-latest` migration to Ubuntu 26.04; the image can be upgraded deliberately after validation.

The repository currently has no branch protection or rulesets. Required checks can be selected after review; the existing sanity check name is preserved.
