# Django Rusty Templates (Agentic)

Django Rusty Templates (Agentic) is a fork of [Lily's Django Rusty Templates library](https://github.com/LilyFirefly/django-rusty-templates), an experimental reimplementation of Django's templating language in Rust. This fork began as an agentic attempt to try out the idea, exploring compatibility, performance, and shared server/browser templates.

The distribution is named `django-rusty-templates-agentic` to respect the original project's PyPI namespace. It is intended as a drop-in replacement while upstream develops: Python imports remain `django_rusty_templates`, and the Django backend remains `django_rusty_templates.RustyTemplates`. Lily and the upstream contributors retain credit for the original library; see [LICENSE](LICENSE).

Template compatibility is tested against Django 5.2, 6.0, and 6.1. Django 6 requires Python 3.12 or newer. Partial templates, inheritance, and fully custom Python tags use Django's parser/node interoperability layer; compatible variables use the native renderer. Django 6.2 development support is tested against a pinned `main` checkout; its `now` format expressions also use the interoperability layer. See [CONTRIBUTING.md](CONTRIBUTING.md) for local conformance checks.

## Goals

* 100% compatibility of rendered output.
* Error reporting that is at least as useful as Django's errors.
* Improved performance over Django's pure Python implementation.

## Versioning

Releases use calendar versioning: `YYYY.MM.INCR`, with the increment starting at `1` each month. The current package version is `2026.10.1`.

## Installation

Install this fork from its GitHub repository or a local checkout. These instructions do not require a PyPI release.

Because both distributions provide the same Python package, uninstall the original before installing this fork in an existing environment:

```sh
pip uninstall django-rusty-templates
pip install git+https://github.com/jefftriplett/django-rusty-templates-agentic.git
```

For a local installation:

```sh
git clone https://github.com/jefftriplett/django-rusty-templates-agentic.git
cd django-rusty-templates-agentic
pip install .
```

Source installations require a [Rust compiler](https://rustup.rs/). To switch back to upstream, uninstall `django-rusty-templates-agentic` before installing the original distribution. Your imports and backend setting stay the same.

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
For our AI/LLM policy, see [AI_POLICY.md](AI_POLICY.md).
