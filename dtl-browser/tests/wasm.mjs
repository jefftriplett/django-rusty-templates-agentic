// Exercise the same generated web bindings used by the browser demo.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { performance } from "node:perf_hooks";
import init, { BrowserEngine } from "../pkg/dtl_browser.js";

const bytes = await readFile(new URL("../pkg/dtl_browser_bg.wasm", import.meta.url));
const start = performance.now();
await init({ module_or_path: bytes });
const initializationMs = performance.now() - start;
const cases = JSON.parse(await readFile(new URL("./parity.json", import.meta.url), "utf8"));
cases.push(JSON.parse(await readFile(new URL("../demo/catalog.json", import.meta.url), "utf8")));
for (const test of cases) {
  const engine = new BrowserEngine();
  try {
    for (const [name, source] of Object.entries(test.templates ?? {})) engine.addTemplate(name, source);
    engine.addTemplate(test.name, test.source);
    assert.equal(engine.render(test.name, JSON.stringify(test.context)), test.expected, test.name);
  } finally {
    engine.free();
  }
}
const engine = new BrowserEngine();
try {
  assert.throws(() => engine.addTemplate("bad", "{% load unsupported %}"), /Unsupported tag/);
  assert.throws(() => engine.render("missing", "{}"), /Template not found/);
  engine.addTemplate("cycle", "{% include 'cycle' %}");
  assert.throws(() => engine.render("cycle", "{}"), /depth/);
  engine.addTemplate("good", "{{ name }}");
  assert.throws(() => engine.render("good", "[]"), /JSON object/);
  assert.equal(engine.render("good", '{"name":"<&"}'), "&lt;&amp;");
} finally {
  engine.free();
}
console.log(JSON.stringify({ parityCases: cases.length, initializationMs, wasmBytes: bytes.length }));
