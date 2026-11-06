# Django Rusty Templates (Agentic)

Django Rusty Templates (Agentic) is a fork of [Lily's Django Rusty Templates library](https://github.com/LilyFirefly/django-rusty-templates), an experimental reimplementation of Django's templating language in Rust. This fork began as an agentic attempt to try out the idea, exploring compatibility, performance, and shared server/browser templates.

The distribution is named `django-rusty-templates-agentic` to respect the original project's PyPI namespace. It is intended as a drop-in replacement while upstream develops: Python imports remain `django_rusty_templates`, and the Django backend remains `django_rusty_templates.RustyTemplates`. Lily and the upstream contributors retain credit for the original library; see [LICENSE](LICENSE).

Template compatibility is tested against Django 5.2, 6.0, 6.1, and the development version (Django's `main` branch, which will become 6.2). Django 6 requires Python 3.12 or newer. Supported templates, including `extends` and `block`, compile and execute inheritance in Rust in both debug modes. Rust manages block overrides, `block.super`, inheritance history, render state, and block scopes; ordinary variables read native context storage. Python contexts are exported at callback, signal, loader, and unsupported-template boundaries. A compatibility bridge preserves mixed native/custom-template inheritance. Partial templates and fully custom Python tag compilers use Django's parser/node interoperability layer. In both debug modes, mixed templates compile supported loops, conditions, variables, includes, URLs and standard helper tags into Rust regions. Opaque custom nodes execute against the original live Django Context at explicit AST boundaries; mixed inheritance keeps its Django block protocol. In debug mode, errors inside a region report the failing token of the full template, as Django does. On the development version, `{% now %}` format variables and filters also render natively. See [CONTRIBUTING.md](CONTRIBUTING.md) for local conformance checks.

To measure inheritance compilation separately from warm rendering and uncached loading plus rendering, run `uv run maturin develop --release` followed by `uv run python scripts/benchmark_inheritance.py`. The benchmark checks output equality and compares Django, native compilation, and the hybrid parser path on a synthetic template; it does not measure application request latency.

## Goals

* 100% compatibility of rendered output.
* Error reporting that is at least as useful as Django's errors.
* Improved performance over Django's pure Python implementation.

## Versioning

Releases use calendar versioning: `YYYY.MM.INCR`, with the increment starting at `1` each month. The current package version is `2026.11.1`.

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

Add an entry to your [`TEMPLATES` setting](https://docs.djangoproject.com/en/5.2/ref/settings/#std-setting-TEMPLATES) with `"BACKEND"` set to `"django_rusty_templates.RustyTemplates"`:

```python
TEMPLATES = [
    {
        "BACKEND": "django_rusty_templates.RustyTemplates",
        ... # Other configuration options
    },
]
```

### What is ported

**Ported to Rust:**

* **Built-in tags.** All of Django's built-in tags (`defaulttags` and `loader_tags`), including `querystring`, `partialdef` and `partial` on the Django versions that have them.
* **Built-in filters.** All 57 of Django's built-in filters.
* **Stock library tags.** The tags from `{% load i18n %}`, `{% load l10n %}`, `{% load tz %}`, `{% load static %}` and `{% load cache %}`.

Checked against Django 5.2 and the development version with `engine.explain_template()`; see [Inspecting templates](#inspecting-templates).

**Not ported.** These run Django's Python code for every call:

| Library | Filters and tags | Notes |
|---|---|---|
| `{% load l10n %}` | `localize`, `unlocalize` filters | [#281](https://github.com/jefftriplett/django-rusty-templates-agentic/issues/281) |
| `{% load tz %}` | `localtime`, `utc`, `timezone` filters | [#281](https://github.com/jefftriplett/django-rusty-templates-agentic/issues/281) |
| `{% load i18n %}` | `language_name`, `language_name_local`, `language_name_translated`, `language_bidi` filters | [#281](https://github.com/jefftriplett/django-rusty-templates-agentic/issues/281) |
| `django.contrib` | `humanize` filters (`intcomma`, `naturaltime`, …), admin tags, and other contrib libraries | Run as custom libraries. |
| Your project | Custom filters and tags | See below. |

Custom filters run in Python and are called as Django calls them: `is_safe`, `needs_autoescape` and `expects_localtime` are honoured, and quoted string arguments arrive as `SafeString`. `simple_tag` and `inclusion_tag` tags are called from Rust. Tags with their own compile function use Django's parser for that tag, and the rest of the template stays native.

**Ported, but calls Django.** These are parsed and rendered in Rust, then call Django APIs for their results:

* `url` uses the URL resolver, `static` storage, `cache` the cache, `csrf_token` CSRF, and `translate`/`blocktranslate` translations.
* `csp_nonce_attr` (Django 6.1+) calls Django's tag function.

**Ported, with fallbacks for unusual input.** Some built-in filters hand only that input to Django, where Python or Django behave differently between versions:

* `urlize` and `urlizetrunc`: words that might be links.
* `striptags`, `truncatechars_html` and `truncatewords_html`: HTML with entities, comments or loosely formed tags.
* `json_script`: dates, decimals and lazy strings.
* `stringformat`, `dictsort`, `unordered_list` and `floatformat`: unusual format specifiers, subclasses and generators.
* The date formatter: time zones other than `datetime.timezone` and `ZoneInfo`.

The WebAssembly engine supports a smaller subset; see [its documentation](dtl-browser/README.md).

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

### Benchmarks

`uv run python scripts/benchmark_loop_values.py --json-output results.json`
measures integer loops, text-only loops, repeated variables, and mixed component
loops against Django.
It checks output before timing and covers `from_string`, warm loaded rendering,
and loading plus rendering with explicit cached/uncached loaders and both debug
settings. Build the extension with `uv run maturin develop --release` first.
These are synthetic timings, not HTTP request measurements. Use `--baseline`
with a previous JSON report to fail on regressions above 20%.

Run performance comparisons locally on a controlled machine, using matching
Python dependencies and release builds for both revisions. GitHub Actions runs
correctness checks, not performance comparisons. Correctness tests also reject
full Django Context construction during native loop value formatting.

`uv run python scripts/benchmark_callback_contexts.py --json-output callbacks.json`
measures read-only, mutating, retaining and failing captured callables with small
and large contexts, using flat and scoped templates through `from_string` and
filesystem loaders. Plain string/integer controls include sparse and dense flat
templates so avoiding imports cannot hide a slower native variable path. Flat templates with variables or filters can read an exact
Django `Context` directly when a context dictionary has at least 64 bindings and at least eight bindings
per rendering node;
this avoids importing unrelated bindings and making callable snapshots. Retained
dictionaries are the caller's live dictionaries, as in Django. Scoped templates,
context subclasses and request contexts keep their existing synchronization path.
Use `--baseline` locally to compare these cases with a 15% regression gate.

`uv run python scripts/benchmark_render_output.py --json-output output.json`
also tracks block and `with` scopes with small and large caller contexts.
Each case runs in fresh processes with matching Django controls; use
`--baseline` to check a later revision locally against a 15% regression gate.

`uv run python scripts/benchmark_realistic_page.py --json-output page.json`
renders a realistic page through the `DjangoTemplates` and `RustyTemplates`
backends: a base template with `extends`, 25 included cards, Django `User`
objects, model-style methods, i18n tags and common filters, with and without
a request and seven context processors. Templates live in
`scripts/realistic_page/`. Each case checks output against Django and runs in
fresh processes; `--baseline` applies a 15% regression gate. A separate process
then uses `tracemalloc` to report memory growth across repeated renders and
the memory retained by discarded engines, for Django and Rusty. The run fails
if discarded engines leave GC-tracked objects behind.

### Inspecting templates

`engine.explain_template(source, template_name=None)` reports `mode` (`native`, `hybrid`, or
`unsupported`) and the first native compilation blocker. For a tag blocker it
includes `reason`, the complete `tag` token, its UTF-8 byte `span` (start, length),
and its one-based `line`. Version-specific variable syntax reports no tag
location. Inspection loads tag libraries but does not compile a Python fallback
tree or render the template; a native report does not imply zero Python callbacks.

Mixed templates expose the original nodes through `template.nodelist` for Django
inspection and direct indexing. `template.nodelist.rendering_nodes` contains the
compiled runtime regions. Inspecting Python nodes does not mean their render
methods execute; the component regression tests disable those methods to verify
Rust dispatch. Region source spans preserve UTF-8 offsets without rewriting tags.

### Isolating custom tag compilers

Rust can keep the surrounding template native while Django compiles an opaque
custom tag and the body it consumes. A compiler can opt in with
`compile_tag.rusty_isolated_compiler = True` (set the attribute on the registered
compiler function, or on a callable compiler object). For example:

```python
@register.tag("card")
def compile_card(parser, token):
    body = parser.parse(("endcard",))
    parser.delete_first_token()
    return CardNode(body)

compile_card.rusty_isolated_compiler = True
```

Opt in only when the compiler uses the parser within its consumed span. The
adapter supports nested `parse()`, `compile_filter()`, `next_token()`,
`prepend_token()`, `delete_first_token()`, loaded tags and filters, custom parser
attributes, and `extra_data`. It preserves tokens, line numbers, DEBUG positions,
origin source names and template names, compiled node identities, and Django
context scopes during rendering.
Native nodes surrounding the custom span never enter Django's parser. A load
inside the opaque body also makes its filters available to following native
nodes. New or replacement custom compilers installed through `parser.tags` must
opt in too; registering an incompatible compiler raises a clear error instead
of rerunning the first compiler on the compatibility path.

Leave the compiler unannotated when it inspects prior native nodes, depends on
`command_stack` outside its own body, reads `parser.origin.loader`, rewrites
future tokens, returns inheritance
or `must_be_first` nodes, or changes how native built-in tags compile. The full
Django parser remains the default for such compilers. Templates sharing
inheritance, named-cycle, or partial registries also retain the compatibility
path, even with an opted-in compiler. Mixed opted-in and unannotated custom tags
are checked before compilation so the opted-in compiler isn't called twice.
`Engine.explain_template()` reports custom compiler boundaries without executing
compilers. Explicit `.nodelist` inspection of a complex template still parses a
separate Django tree and may invoke compilers again; its nodes are distinct from
the nodes used for rendering. Rendering never requests this inspection tree.
