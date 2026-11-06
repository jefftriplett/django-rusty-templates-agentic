# What to expect

How much faster Rusty renders depends on what your templates do. Templates made of loops, variables, built-in filters, inheritance and includes run in Rust and are several times faster than Django. Work that stays in Python runs at Python speed: your own tags and filters, model methods, and some inputs that are handed to Django. Rusty also has costs Django doesn't, so a few cases are slower.

The numbers below come from `scripts/benchmark_summary.py` and `scripts/benchmark_realistic_page.py`. They were measured on an Apple Silicon Mac with Python 3.15, Django 5.2.17 and a release build. Times are per render, best of 7 runs, and both engines produce the same output in every case. They are synthetic template timings, not HTTP request timings. Your own numbers will differ, so measure your own pages (see [Measuring your own project](#measuring-your-own-project)).

## Where it is faster

Warm renders with the cached loader, which both backends use by default when you don't set `loaders`.

| Scenario | Django | Rusty | Speedup |
|---|---|---|---|
| Realistic page: `extends`, 25 included cards, `User` objects, i18n tags, a request and 7 context processors | 2,941 µs | 404 µs | 7.3× |
| Table, 200 rows: `title`, `lower`, `date`, `floatformat`, `default`, `truncatechars`, `cycle`, `if` | 5,730 µs | 437 µs | 13.1× |
| Inheritance: `extends`, `block.super`, 20 includes | 193 µs | 30 µs | 6.5× |
| 200 model-style objects: properties, methods, related attributes | 1,612 µs | 259 µs | 6.2× |
| Custom filter on each of 200 rows | 341 µs | 65 µs | 5.3× |
| Mixed template: one custom `Node` tag, rest native | 1,947 µs | 536 µs | 3.6× |
| Tiny template, two variables | 4.7 µs | 1.8 µs | 2.7× |
| Tiny template, 100 context keys | 5.1 µs | 2.0 µs | 2.5× |
| Tiny template, 500 context keys | 5.1 µs | 2.9 µs | 1.7× |

Built-in filters in a loop gain the most. Each filter is 7–17× faster than Django's, apart from `urlize` (below). This includes the `{% load l10n %}`, `{% load tz %}` and `{% load i18n %}` filters, and printing dates, times and `Decimal`s. Keys the template never reads cost almost nothing, because a context `dict` is read lazily. Python attribute access and method calls still run in Python, which is why model-heavy templates gain less than plain data.

Compiling is also much faster, which matters for the first request after a deploy and for development without the cached loader:

| Compile only | Django | Rusty | Speedup |
|---|---|---|---|
| Table template | 114 µs | 6.0 µs | 19× |
| Inheritance page | 54 µs | 3.7 µs | 15× |
| Tiny template | 9.9 µs | 1.2 µs | 8× |

`DEBUG = True` warm renders are as fast as non-debug ones: 13.3× for the table, 3.7× for the mixed template.

## Where it is not faster

| Scenario | Django | Rusty | Speedup |
|---|---|---|---|
| Compile a mixed template (custom `Node` tag) | 50 µs | 71 µs | 0.70× |
| `urlize` on 200 email addresses | 2,720 µs | 2,304 µs | 1.18× |
| Custom `simple_tag` on each of 200 rows | 322 µs | 191 µs | 1.7× |

- **Mixed templates compile slower.** A custom tag with its own compile function makes the template "mixed". Django's parser then builds that template, and Rust compiles the native regions on top. Warm renders are still several times faster, so this only matters without the cached loader.
- **Inputs handed to Django.** `urlize` sends any word that might be a link to Django's `urlizer`, which keeps validators and IDNA handling exact. Text full of email addresses or URLs therefore runs at about Django's speed. The README lists the other fallbacks under "What is ported".
- **Python callbacks.** Custom tags and filters run in Python. Simple tags gain least, because each call goes from Rust into Python and back.

## What to expect in production

A template speedup only shortens the part of a request spent rendering. Database queries, view code, middleware and serialization don't change. If rendering takes a fraction *p* of a request and becomes *s* times faster, the whole request becomes `1 / ((1 − p) + p / s)` times faster:

| Rendering share of the request | 3× faster templates | 6× faster templates | 10× faster templates |
|---|---|---|---|
| 10% | 1.07× | 1.09× | 1.10× |
| 25% | 1.20× | 1.26× | 1.29× |
| 50% | 1.50× | 1.71× | 1.82× |
| 75% | 2.00× | 2.67× | 3.08× |

Pages that do little else besides rendering gain the most: large lists, tables, reports, sitemaps and feeds. API views, and pages dominated by database queries, gain little.

Things that affect what you will see:

- **Keep the cached loader.** Both backends wrap their loaders in it unless you set `loaders` yourself. Without it, every render reads and compiles the template, though Rusty compiles native templates much faster.
- **Check which templates are native.** `engine.explain_template(source)` reports `native`, `hybrid` or `unsupported` and the first blocker; see [Inspecting templates](../../README.md#inspecting-templates). Hybrid templates still render faster, but compile slower.
- **Hot custom tags and filters stay in Python.** If profiling shows one custom filter or tag dominating, moving its work into the view, or using a built-in, gains more than the engine can.
- **Database queries triggered from templates** take the same time with either engine, for example a lazy queryset first evaluated in `{% for %}`.
- **Memory.** In `benchmark_realistic_page.py`, after the first window, Rusty's traced Python memory grew 1.5–7 KB per window, about the same with 200 or 1,000 renders per window. So it isn't growing with each render. Django's grew under 1 KB. A discarded Rusty engine retains less Python memory than Django's (about 1,000 vs 1,300 bytes). `tracemalloc` doesn't see Rust's own allocations, so watch process memory in long-running workers too.

## Measuring your own project

Profile real requests first, for example with Django Debug Toolbar or `cProfile`, to see how much time goes to rendering. Then compare the two backends on your own templates and contexts. Switch `BACKEND` between `django.template.backends.django.DjangoTemplates` and `django_rusty_templates.RustyTemplates`, then time `get_template(name).render(context, request)` for the pages you care about. Use a release build (the published wheels are release builds), and compare on the same machine, with the same data, one after the other.

Run the bundled scenarios with:

```bash
uv run maturin develop --release
uv run python scripts/benchmark_summary.py --json-output summary.json
uv run python scripts/benchmark_realistic_page.py --json-output page.json
```

Run performance comparisons locally, not in CI. Shared CI runners are too noisy for timing.
