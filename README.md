# Django Rusty Templates (Agentic)

Django Rusty Templates (Agentic) is a fork of [Lily's Django Rusty Templates library](https://github.com/LilyFirefly/django-rusty-templates), an experimental reimplementation of Django's templating language in Rust. This fork began as an agentic attempt to try out the idea, exploring compatibility, performance, and shared server/browser templates.

The distribution is named `django-rusty-templates-agentic` to respect the original project's PyPI namespace. It is intended as a drop-in replacement while upstream develops: Python imports remain `django_rusty_templates`, and the Django backend remains `django_rusty_templates.RustyTemplates`. Lily and the upstream contributors retain credit for the original library; see [LICENSE](LICENSE).

Template compatibility is tested against Django 5.2, 6.0, and 6.1. Django 6 requires Python 3.12 or newer. Supported templates, including `extends` and `block`, compile and execute inheritance in Rust in both debug modes. Rust manages block overrides, `block.super`, inheritance history, render state, and block scopes; ordinary variables read native context storage. Python contexts are exported at callback, signal, loader, and unsupported-template boundaries. A compatibility bridge preserves mixed native/custom-template inheritance. Partial templates and fully custom Python tag compilers use Django's parser/node interoperability layer. Django 6.2 development support is tested against a pinned `main` checkout; its `now` format expressions also use the interoperability layer. See [CONTRIBUTING.md](CONTRIBUTING.md) for local conformance checks.

To measure inheritance compilation separately from warm rendering and uncached loading plus rendering, run `maturin develop --release` followed by `.venv/bin/python scripts/benchmark_inheritance.py`. The benchmark checks output equality and compares Django, native compilation, and the hybrid parser path on a synthetic template; it does not measure application request latency.

## Goals

* 100% compatibility of rendered output.
* Error reporting that is at least as useful as Django's errors.
* Improved performance over Django's pure Python implementation.

## Versioning

Releases use calendar versioning: `YYYY.MM.INCR`, with the increment starting at `1` each month. The current package version is `2026.10.4`.

To release, run `just release-prepare` from a clean `main` checkout. It uses
[bumpver](https://github.com/mbarkhau/bumpver) to pick the next version, updates
`bumpver.toml`, `Cargo.toml`, `Cargo.lock`, and this version notice, commits the
bump on a release branch, pushes it, and opens a PR. Extra arguments go to
`bumpver update`; for example, `just release-prepare --set-version 2026.11.1`.
Once the PR passes checks and is merged, switch to `main` and run `just release`.
This pulls the merged version and pushes an annotated tag, triggering the wheel
builds and PyPI publishing in GitHub Actions. Check the **Build wheels** workflow
for completion.

## Installation

Install the published package from [PyPI](https://pypi.org/project/django-rusty-templates-agentic/).

With [uv](https://docs.astral.sh/uv/), create a virtual environment if you don't already have one, then install:

```sh
uv venv
uv pip install django-rusty-templates-agentic
```

Or with pip in your Python environment:

```sh
pip install django-rusty-templates-agentic
```

Because both distributions provide the same Python package, uninstall the original before installing this fork in an existing environment:

```sh
uv pip uninstall django-rusty-templates
uv pip install django-rusty-templates-agentic
```

Or with pip:

```sh
pip uninstall django-rusty-templates
pip install django-rusty-templates-agentic
```

For a source installation from GitHub:

```sh
uv pip install git+https://github.com/jefftriplett/django-rusty-templates-agentic.git
```

Or with pip:

```sh
pip install git+https://github.com/jefftriplett/django-rusty-templates-agentic.git
```

Or install from a local checkout:

```sh
git clone https://github.com/jefftriplett/django-rusty-templates-agentic.git
cd django-rusty-templates-agentic
```

Then run `uv pip install .` (after `uv venv` if needed) or `pip install .`.

Prebuilt wheels are used when available for your platform and Python version. Installing from a source distribution or checkout requires a [Rust compiler](https://rustup.rs/). To switch back to upstream, uninstall `django-rusty-templates-agentic` before installing the original distribution. Your imports and backend setting stay the same.

## Usage

Add an entry to your [`TEMPLATES` setting](https://docs.djangoproject.com/en/5.1/ref/settings/#std-setting-TEMPLATES) with `"BACKEND"` set to `"django_rusty_templates.RustyTemplates"`:

```python
TEMPLATES = [
    {
        "BACKEND": "django_rusty_templates.RustyTemplates",
        ... # Other configuration options
    },
]
```

## WebAssembly for JavaScript clients

This fork also includes an experimental WebAssembly (Wasm) version for JavaScript clients. Templates render directly in the browser using the `BrowserEngine` JavaScript API, with JSON context data and support for variables, selected filters, conditionals, loops, includes, and inheritance.

The browser engine and the opt-in Python `PortableEngine` share the same Rust parser and renderer, allowing supported templates to run unchanged on both the server and client. The Wasm version supports a subset of Django's template language; the existing `RustyTemplates` backend retains its full Django implementation.

See the [WebAssembly documentation](dtl-browser/README.md) for build instructions, the JavaScript API, a working demo, and the supported features and limitations.

## Contributing

Django Rusty Templates (Agentic) is open to contributions. These can come in many forms:

* Implementing missing features, such as filters and tags built into Django.
* Reporting bugs where Django Rusty Templates gives the wrong result.
* Adding new test cases to ensure Django Rusty Templates behaves the same as Django.
* Adding benchmarks to track performance.
* Refactoring for readability or performance.

For detailed instructions on setting up your development environment and contributing to the project, please see [CONTRIBUTING.md](CONTRIBUTING.md).
