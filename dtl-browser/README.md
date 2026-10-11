# Browser WebAssembly prototype

An experiment on `prototype/browser-wasm`, consolidating the browser prototype
and shared-core inheritance work. The Python-free `dtl-core` crate now owns the portable parser and
renderer, including inheritance. Thin Python `PortableEngine` and browser
`BrowserEngine` adapters use that exact same core. The existing production
`Engine` / `RustyTemplates` backend keeps its full Django implementation. This is a feasibility prototype,
not a replacement for the Django backend or a claim of full DTL compatibility.

## Build and open the demo

From the repository root, with the repository's Rust toolchain:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
bash dtl-browser/build.sh
python3 -m http.server 8000 --bind 127.0.0.1 --directory dtl-browser
```

Open <http://127.0.0.1:8000/demo/index.html>. Edit the page, base template, included
card and JSON data, then click Render template. The initial bundle is loaded from
`demo/catalog.json`, the same source/data fixture tested on all server runtimes. The renderer runs entirely in the browser;
the local HTTP server only serves static files. HTTP is required for loading the
Wasm module; opening the HTML file directly is insufficient.

Generated JavaScript, TypeScript declarations and Wasm are in `dtl-browser/pkg/`,
which is ignored by Git. The build script checks that the CLI matches the pinned
binding dependency. No npm packages are needed to build or run the demo.

## JavaScript API

```js
import init, { BrowserEngine } from "./pkg/dtl_browser.js";
await init();
const engine = new BrowserEngine();
try {
  engine.addTemplate("card", "<li>{{ row.name }}</li>");
  engine.addTemplate("page", '{% for row in rows %}{% include "card" %}{% endfor %}');
  const html = engine.render("page", JSON.stringify({ rows: [{ name: "Books & more" }] }));
  // <li>Books &amp; more</li>
} finally {
  engine.free();
}
```

`addTemplate` parses once, replacing the named template only after successful
compilation. Includes resolve against the current registry at render time.
`render` accepts a JSON object encoded as a string and returns an HTML string.
Errors throw strings across the binding. Free engine instances when finished.
This API generates HTML; it does not implement DOM reconciliation or reactivity.

## Use the same bundle on the server

The rebuilt Python extension exports an experimental, opt-in `PortableEngine`:

```python
import json
from pathlib import Path
from django_rusty_templates import PortableEngine

bundle = json.loads(Path("dtl-browser/demo/catalog.json").read_text(encoding="utf-8"))
engine = PortableEngine()
for name, source in {**bundle["templates"], bundle["name"]: bundle["source"]}.items():
    engine.add_template(name, source)
html = engine.render_json(bundle["name"], json.dumps(bundle["context"]))
assert html == bundle["expected"]
```

Rebuild with `.venv/bin/maturin develop` first. This adapter accepts the same
JSON data representation as browser `render`, and raises `ValueError` for core
errors. It is not automatically selected by the Django backend and does not
replace existing Python object/tag/filter behavior. The shared example is also
rendered unchanged by the **existing** Rusty Django backend and Django itself.

## Supported subset

- Text, inline comments, variable output and HTML autoescaping.
- JSON object keys and array indices with dotted lookups; missing output is empty.
  JSON null renders as `None`, booleans as `True`/`False`.
- Quoted strings, numeric literals, `True`, `False`, `None`, and filter chains.
- Filters: `upper`, `lower`, `length`, `default`, `default_if_none`, `join`,
  `escape`, `safe`. Arguments can be literals or dotted data lookups.
- `if`/`elif`/`else` with `and`, `or`, `not`, comparisons and `in`/`not in`.
  Operators must be separated by whitespace; boolean evaluation short-circuits.
- Single-variable `for`, `reversed`, `empty`, strings/arrays/object keys,
  counters and nested `forloop.parentloop`; loop scopes are restored.
- `include` with a literal or dynamic name and the current context.
- `extends`, named `block`/`endblock`, dynamic/filtered parent names, nested blocks,
  multiple inheritance levels and lazy, escaped-once `block.super`. Relative parent
  and include paths resolve within the registered template namespace; includes
  use an independent inheritance block map.

Unsupported tags and filters fail during registration rather than silently
falling back to Python. Unsupported rendering operations also return errors.

## Limits and deliberate differences

No custom Python or JavaScript tags, callable/attribute
protocol, request/context processors, translation, localization/timezones,
`include with/only`, unpacking loops, `autoescape off`, or general Django filters.
Cross-include `block.super` references and recursive same-name loader/origin
selection are not implemented. Only in-memory named templates are supported.
Direct JSON array/object output is rejected. JSON object iteration retains input
order. JSON/JavaScript numbers and Unicode casing do not promise every Python
numeric/string edge case. Only the shared fixtures below establish parity; the
full Django suite has **not** been ported to this runtime.

Templates/context strings and rendered output are limited to 1 MiB, nesting/inheritance to
32, conditions/filter chains to 64 terms/filters, and rendering to 100,000 steps.
These are experimental guardrails, not a comprehensive resource sandbox.
Templates should be trusted application assets. `safe` deliberately bypasses
escaping. The demo preview uses a sandboxed iframe; consumers must decide how to
insert the resulting HTML into their own application.

## Verification

```sh
cargo test -p dtl-browser
.venv/bin/python dtl-browser/tests/check_django.py
bash dtl-browser/build.sh
node dtl-browser/tests/wasm.mjs
.venv/bin/pytest -q tests/test_portable_templates.py
```

The same 39 fixtures (including the actual demo bundle) are checked against
Django, the existing Rusty server backend, the new shared-core Python adapter,
native Rust and generated Wasm/JavaScript bindings. Rust tests also cover unsupported syntax, invalid JSON,
recursive includes, failed replacement, output limits and work limits.

Optional real-browser verification uses Playwright, kept local to this crate:

```sh
cd dtl-browser
npm ci
npx playwright install chromium
npm run test:browser
```

For an existing Chrome installation, set `CHROME_PATH` to its executable instead
of installing Chromium. The test starts/stops its own local static server, checks
the demo and iframe, runs the shared fixtures in Chrome, checks error recovery,
and records a 100-row render benchmark in `target/dtl-browser/browser-results.json`.
No additional CI jobs or browser matrix have been enabled.

## Initial measurements

Measured locally on macOS ARM64, Python-free Wasm in headless Chrome 150:

- Generated Wasm: approximately 167 KiB before compression (binding JS additional).
- 100-row loop: approximately 278 µs median, including JSON parsing, lookups,
  escaping and the JavaScript/Wasm boundary; 15 samples of 200 renders after warmup.

This is a synthetic loop benchmark, not an application benchmark or a browser
speedup claim. Initialization/network/DOM costs are excluded from the render
timing. Existing native Python-wheel timings must not be assumed to apply here.

Before considering a shared production core, measure an actual application's
templates, data conversion costs and startup/download costs. This branch
demonstrates the usual `base.html` pattern without weakening the existing Django
backend. It does not migrate the full backend to this experimental core.

The production compatibility gate remains the unchanged Django 5.2/6.0/6.1
conformance suites plus pinned 6.2 development coverage, together with the existing
project tests. Browser subset coverage is reported separately from those results.
