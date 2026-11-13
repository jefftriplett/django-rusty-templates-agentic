# Changelog

## Unreleased

Raise `RecursionError`, as Django does, when a template includes itself or a recursive include runs out of stack, instead of crashing the process; `center` with a huge width and `forloop.length` as a filter argument before Django 6.0 no longer abort or panic either. ([#373](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/373))

Make `{% if %}` follow Django: exceptions from a bare condition (such as a database error in `{% if queryset %}`) are raised instead of rendering the else branch, `and`/`or` only evaluate their right side when needed, and `!=` calls `__ne__`. A missing filter argument now raises even when the filter wouldn't use it. ([#374](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/374))

Match Django's output for `title` (`ab 'cd'`, non-ASCII text), `slugify` (leading underscores, number literals), `wordwrap` (non-ASCII text, runs of spaces, tabs), `last` on safe text, and float literals passed to string filters. ([#375](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/375))

Render 35–80% faster: `Template.render` no longer calls `mark_safe` in Python, copies the caller's dict or hashes names with SipHash, dotted lookups such as `{{ o.owner.city }}` reuse interned names, and wheels build with link-time optimisation. A dict passed to `render` is now read live, as Django reads it. ([#383](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/383), [#384](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/384), [#387](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/387))

Close a `{% for %}` loop's scope when it raises, so a tag that catches the error and keeps rendering no longer sees the failed loop's variables or `forloop`. ([#385](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/385))

Compiled templates take less memory: a `{{ v }}` node drops from about 235 bytes to 66, and an empty template from 1.5 KB to 0.7 KB. ([#386](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/386))

Reuse one handle per compiled template instead of building a new one on every render, and recognise Django contexts by type: printing a variable or plain text is 20–30% faster. ([#389](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/389))

Cache number and date formats across renders while still following `override_settings`, the active language and `USE_THOUSAND_SEPARATOR`: printing a number drops from 4.5 µs to 1 µs, so a template printing one number is no longer slower than Django. ([#390](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/390))

Stop copying the block table into every `{% block %}` and `{% with %}`, which made templates with many blocks quadratic: 80 overridden blocks render in 64 µs instead of 601 µs. ([#392](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/392))

Read a plain Django `Context` passed to `render` lazily instead of flattening it, and let callables see it directly: these renders are 2–11× faster and now faster than Django. ([#393](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/393))

Handle list and tuple indexes, missing attributes, printing objects and `None`, `escape`, `simple_tag` results, string `add`, `widthratio` and `spaceless` natively, 1.5–5× faster than before. `{{ " 1"|add:"2" }}` now gives `3` and `{{ obj|escape }}` escapes `str(obj)`, as in Django. ([#394](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/394))

A template with a custom compiled tag reads and writes Django's `Context` natively, so it renders about 4× faster than Django instead of 1.4×; `forloop` keys now come in Django's order. ([#395](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/395))

Fix a hang when a value's `__del__` writes to the context it is being replaced in, and stop holding locks while `block.super` renders or a cached value is built. ([#396](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/396))

Give filters Django's value for a missing variable (`""`, or `None` in `{% if %}`), so `{{ missing|add:text }}` or `{{ missing|length }}` render what Django renders; `cut` and `yesno` raise Django's exception types, and Django 5.2 rejects `{{ 1e-7 }}` as Django does. ([#397](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/397))

A template that a tag renders with its context no longer sees the outer template's block overrides, so its own `{% extends %}` and blocks work as in Django. ([#398](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/398))

In templates with a custom compiled tag, `{% for %}` loops that contain no custom tags or includes now render natively when no Python code can reach the Django `Context`, making a template with one custom tag about 2.5× faster and use about a third of the memory. ([#400](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/400))

Parse templates that use a custom tag without `rusty_isolated_compiler` about 25% faster (102 → 75 µs against Django's 52) and keep about a quarter less memory per template, by deferring `{% for %}`/`{% if %}` arguments that Rust compiles anyway. ([#402](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/402))

Render a loop with `{% with %}` inside about 6–10% faster by not re-attaching to Python for each loop and scope handle. ([#404](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/404))

Internal cleanups with no change in behavior: one comparison path for every `{% if %}` operator, and `parse.rs` and `render/filters.rs` split into smaller files. ([#403](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/403), [#405](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/405), [#406](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/406))

Add an `isolated_tags` engine option so a project can mark third-party tags, such as crispy-forms' `crispy` or widget-tweaks' `render_field`, as isolated compilers from `TEMPLATES` instead of patching the packages; names that don't exist raise `ImproperlyConfigured`. ([#408](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/408))

Fix a hang and two panics: a `{% for %}` over an iterator that keeps raising now raises its first error instead of running until memory runs out, `{% now %}` accepts non-ASCII characters around its format, and `{%}` is plain text. ([#420](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/420))

`{% if x is 5 %}` is true for an int equal to 5, as in Django, `length` and `{% firstof %}` no longer swallow errors Django raises, and a missing attribute no longer calls `str()` on the object. ([#421](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/421))

`{% with %}` and `{% block %}` no longer copy the render context, and `{% include … only %}` keeps the render's caches, so filters inside them are two to four times faster. ([#423](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/423))

Match Django on parser edge cases: `{% cycle 'a'|upper 'b' %}`, `{{ x|default:1_000 }}` and `{{ s|upper |lower }}` now render, and `{{ s|upper| }}`, `{% endif x %}` and `{% for x in reversed %}` now raise `TemplateSyntaxError`. ([#425](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/425))

Filter arguments from the context convert as in Django (`wordwrap` and `center` take floats and `Decimal`s, `yesno` takes a lazy string), custom filters are checked for their argument count when compiling, and a missing variable with `string_if_invalid` set skips every filter. `{{ x|divisibleby:0 }}` and a `{% url %}` mixing positional and keyword arguments now fail when rendered, not when compiled. ([#426](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/426))

The context a `takes_context` simple tag receives gains `push`, `pop`, `update` as a context manager, `setdefault`, `set_upward`, `render_context`, `new` and copying. ([#427](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/427))

`yesno` without an argument, printing `None` or an object, `urlize`, `{% get_current_language %}` and `make_list` on long text are 1.8 to 6 times faster. ([#428](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/428))

`Engine.select_template` accepts any iterable and lists each missing name once in its error, as Django's does. ([#429](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/429))

The test suite runs in about 37 seconds instead of 83, the broken-engine check runs its two modes together, and the weekly mutation workflow builds its baseline again. ([#422](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/422))

A loop variable or assignment named `forloop` replaces the loop object, digits of other scripts are numbers (`{{ ١٢ }}` is 12), and a custom filter given `_("text")` receives a lazy translation, all as in Django. A backend template now raises Django's `TypeError` when given a `Context` instead of a dict. ([#431](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/431))

A `takes_context` simple tag can read `context.template`, `context.template_name` and `context.dicts`, and `isinstance(context, Context)` is true, so tags that call `context.template.engine.get_template(...)` or check the type before `flatten()` work. ([#432](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/432))

`title` on accented text, `striptags` and the HTML truncation filters on text with `&amp;`-style references, and `naturaltime` for dates over a day away no longer call Django's Python code: they are 4 to 37 times faster. Each `{% extends %}` level is about 15% cheaper. ([#433](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/433), [#434](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/434))

Tracing template nodes for Python's garbage collector now names every field, so a new field can't be left untraced by accident; the translation cache on `{% translate %}` and `{% blocktranslate %}` is traced too. ([#435](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/435))

## 2026.11.3

Compile Django admin's inclusion tags, `get_admin_log` and `get_flatpages` natively, so all of Django's contrib templates, including the admin change form, change list and index, compile natively. ([#318](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/318))

Let custom tags marked `rusty_isolated_compiler = True` keep templates that use `extends`, `block`, `cycle` or partials native, unless those tags appear inside the custom tag's body. ([#319](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/319))

Pass the live context to inclusion tags registered with `takes_context=True`, so changes their function makes reach later nodes as in Django. ([#321](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/321))

Stop copying every context variable after each `{% with %}` or inherited `{% block %}` scope, which made loops slow down as the context grew. ([#323](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/323))

Render `{% firstof … as %}`, `{% regroup %}` with a plain key, and `{% querystring %}` with plain keyword values natively. These were slower than Django and are now 7–10× faster. ([#324](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/324), [#325](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/325), [#326](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/326))

Check that tests actually test Rusty: CI now fails when a test still passes with the Rusty engine broken on purpose, or when a new Django conformance test passes by falling back to Django's parser, and `just mutants` adds mutation testing for the Rust code. ([#332](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/332), [#333](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/333), [#334](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/334), [#335](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/335))

Build `urlize` and `urlizetrunc` links in Rust instead of calling Django for each word, making them about 10× faster than Django on emails and URLs. ([#338](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/338))

Render all six `humanize` filters (`intcomma`, `intword`, `ordinal`, `apnumber`, `naturalday` and `naturaltime`) in Rust when they are Django's own, making them 4–39× faster than Django. ([#342](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/342), [#355](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/355))

Localize integers Rusty computes itself, such as `forloop.counter`, integer literals and `add` results, so `USE_THOUSAND_SEPARATOR` and locale separators apply as in Django; `forloop.length` no longer renders on Django 5.2. ([#343](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/343))

Document which Django contrib apps are supported and how each is tested. ([#348](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/348))

Let Rusty's engines and templates be copied and deep-copied, so copying a form's errors works as with Django. ([#350](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/350))

Run Django's contrib and forms test suites (admin, auth, forms, humanize, sitemaps and more) in the conformance checks. ([#351](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/351))

Make `takes_context` simple tags about 1.6× faster than Django, close to plain simple tags, by moving less state on each call and reusing the Python context wrapper. ([#353](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/353), [#356](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/356))

Accept translated filter arguments such as `default:_("Not started")` after a variable in `with`, `if`, `for` and `include`, which Rusty rejected and which broke a Wagtail admin template. ([#359](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/359))

Add `scripts/survey_templates.py` and a [report](docs/third-party-templates.md) on how much of crispy-forms, allauth, Wagtail, debug-toolbar, compressor and widget-tweaks compiles natively, and which of their tags fall back. ([#361](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/361))

## 2026.11.2

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

Let block tags span lines on Django 6.2 and accept its transitional `allow_multiline_tags` option, read 6.2 `comment` blocks as raw text, and close a comment only on an exact `endcomment`, as Django does. ([#311](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/311))

Refresh the benchmark numbers and summarize them in the README, and clarify which printed values still fall back to Python. ([#309](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/309), [#312](https://github.com/jefftriplett/django-rusty-templates-agentic/pull/312))

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
