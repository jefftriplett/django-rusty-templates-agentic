# Changelog

## Unreleased

Report the tried origins when a loader can't find a template and keep the original error as the cause for invalid context processor results and loader configurations, and deny more Clippy restriction lints. ([#207](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/207))

Render `first`, `linenumbers`, `ljust`, and `rjust` natively, and pass `truncatechars_html` the cached truncation text. ([#214](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/214), [#215](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/215), [#216](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/216), [#219](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/219))

Fix the per-language cache, which never cached after #197, so `floatformat`, `truncatechars`, `timesince`, and `timeuntil` use their native paths again. ([#218](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/218))

Render `pluralize`, `get_digit`, `phone2numeric`, `urlencode`, `iriencode`, `make_list`, `escapeseq`, and `safeseq` natively, and call `random` and `pprint` directly. ([#253](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/253), [#254](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/254), [#255](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/255), [#256](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/256), [#257](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/257))

Follow Python's `int()` for integer filter arguments and handle more `join`, `slice`, `truncatewords`, `ljust`, `rjust`, and `center` inputs natively, including `center` on Django 6.0+ and string widths on 5.2. ([#258](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/258))

Read the active translation without asgiref's lock, and render `filesizeformat` and `floatformat` strings and `g`/`u` suffixes natively. ([#260](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/260), [#261](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/261), [#262](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/262))

Render `unordered_list`, `striptags`, `stringformat`, `dictsort`, `dictsortreversed`, and `json_script` natively for common inputs. ([#263](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/263), [#264](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/264), [#265](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/265), [#266](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/266), [#267](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/267))

Format timezone date specifiers natively, compute `timesince` and `timeuntil` for any tzinfo, and correct the README note on `{% now %}` for the development version. ([#269](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/269), [#270](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/270), [#271](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/271))

Truncate non-ASCII text and well-formed HTML natively in `truncatechars`, `truncatechars_html`, and `truncatewords_html`, and handle words that can't become links natively in `urlize` and `urlizetrunc`. ([#272](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/272), [#273](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/273), [#274](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/274))

Render mixed templates with native regions in debug mode, keeping Django's error positions for the full template. ([#275](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/275))

Keep quoted string literals safe under `{% autoescape off %}`, call custom filters with Django's `needs_autoescape`, `is_safe` and `expects_localtime` flags, and list in the README what still runs in Python. ([#283](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/283))

Speed up compiling mixed templates by about 6%, run the `pytz` and `python-dateutil` tests in CI, and restore a test hidden by a duplicate name. ([#280](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/280), [#285](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/285), [#286](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/286))

Compile mixed templates with many variables 2–3× faster by parsing Django's filter expressions for native variables only when something reads them. ([#289](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/289))

List what is ported in the README, and add a summary benchmark with a guide to what performance to expect. ([#291](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/291), [#297](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/297))

Print dates, times, datetimes and `Decimal`s natively, and render the `{% load tz %}`, `{% load l10n %}` and `{% load i18n %}` filters natively, which makes them 8–27× faster than Django. ([#300](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/300), [#301](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/301), [#302](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/302), [#303](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/303), [#304](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/304))

Read context dicts lazily, so a small template with 500 context keys goes from 11× slower than Django to 1.7× faster, accept non-string keys as Django does, and keep `True`, `False` and `None` in `{% include … only %}`. ([#306](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/306), [#307](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/307))

## 2026.11.1

Build macOS wheels on `macos-15` after GitHub retired the `macos-14` runner. ([#206](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/206))

Move shared Python imports to module level while retaining deferred imports for Django-version-specific features. ([#188](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/188))

Refresh outdated README details, including the tested Django versions and benchmark regression gates, and skip pull request builds and tests for docs-only changes. ([#190](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/190))

Render `slice`, `join`, `truncatewords`, `linebreaks`, `linebreaksbr`, `timesince`, `timeuntil`, and `{% now %}` natively, cache translated text and non-ASCII truncation text per language, and benchmark built-in filters in loops. ([#192](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/192), [#193](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/193), [#194](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/194), [#195](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/195), [#196](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/196), [#197](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/197), [#198](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/198), [#199](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/199), [#200](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/200), [#201](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/201))

Key the sanity CI Rust cache on the exact Python version so a Python patch release no longer breaks linking. ([#202](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/202), [#203](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/203))

## 2026.10.14

Enforce workspace lints and simplify builtin filter registration, template caches, and fallback errors. ([#173](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/173), [#174](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/174), [#175](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/175), [#176](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/176))

Fix loader reentry, Django 6 cache updates, and timezone handling; speed up date, number, and string formatting and callable rendering; and add realistic page benchmarks with memory checks. ([#177](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/177), [#178](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/178), [#179](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/179), [#180](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/180), [#181](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/181), [#182](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/182), [#183](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/183), [#184](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/184), [#185](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/185))

## 2026.10.13

Restore callback rendering performance while preserving cyclic collection, and reduce allocations in blocks and includes. ([#171](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/171))

## 2026.10.12

Collect cycles created by custom tags retaining native contexts or block callbacks, including shared scope and loop data. ([#169](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/169))

## 2026.10.11

Release discarded cached engines through Python cyclic GC while preserving retained templates and loaders, and match Django 6+ include-path errors. ([#166](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/166))

## 2026.10.10

Restore Django 6.1 admin rendering with CSP nonce support, complete the default filter set, and fix mixed-template CSRF and HTML handling, Windows template loading, and raw-block parsing. Exercise Rusty directly in the template, admin, and CSP conformance suites. ([#163](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/163))

## 2026.10.9

Make local performance checks less sensitive to allocator state and timing outliers, and remove performance comparisons from GitHub Actions. ([#142](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/142), [#144](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/144))

Reduce rendering allocations, Python lookups, and context copying in loops, filters, components, and template scopes. ([#154](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/154), [#155](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/155), [#156](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/156), [#157](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/157), [#158](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/158), [#159](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/159))

## 2026.10.8

Speed up template parsing and custom tags, reduce string copying and large-context rendering costs, and strengthen performance checks. Match Django’s errors and string behavior for simple-tag results. ([#125](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/125), [#126](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/126), [#127](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/127), [#128](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/128), [#135](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/135), [#136](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/136), [#137](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/137), [#139](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/139))

## 2026.10.7

Reduce Django parsing for DEBUG and mixed inheritance, and parse Django 6 expressions and template partials in Rust. ([#120](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/120), [#121](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/121), [#122](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/122))

## 2026.10.6

Reduce loop and component rendering overhead, with performance checks to catch regressions. ([#106](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/106), [#107](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/107))

## 2026.10.5

Parse and render template inheritance, blocks, and `block.super` in Rust. ([#99](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/99))

## 2026.10.4

Reduce rendering allocations and speed up built-in value lookups. ([#97](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/97))

## 2026.10.3

Reduce include rendering overhead and automate release version bumps. ([#94](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/94), [#95](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/95))

## 2026.10.2

Fix template and context inspection in Django's test client, and add release commands. ([#92](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/92), [#93](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/93))

## 2026.10.1

First Agentic release, with Django 5.2 and 6.x compatibility, template inheritance, custom Python tags, and PyPI wheel publishing. ([#45](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/45), [#46](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/46), [#60](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/60), [#85](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/85), [#90](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/90))
