import assert from "node:assert/strict";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const root = resolve(fileURLToPath(new URL("../", import.meta.url)));
const server = createServer(async (request, response) => {
  try {
    const path = resolve(root, `.${decodeURIComponent(new URL(request.url, "http://localhost").pathname)}`);
    if (!path.startsWith(root + sep)) throw new Error("Invalid path");
    const bytes = await readFile(path);
    const type = path.endsWith(".wasm") ? "application/wasm" : path.endsWith(".js") ? "text/javascript" : path.endsWith(".json") ? "application/json" : "text/html";
    response.writeHead(200, { "Content-Type": type });
    response.end(bytes);
  } catch {
    response.writeHead(404);
    response.end();
  }
});
await new Promise((done) => server.listen(0, "127.0.0.1", done));
let browser;
try {
  browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) });
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(String(error)));
  await page.goto(`http://127.0.0.1:${server.address().port}/demo/index.html`);
  await page.getByRole("button", { name: "Render template", exact: true }).waitFor();
  assert.match(await page.locator("#output").textContent(), /&lt;SPECIAL EDITION&gt;/);
  assert.match(await page.frameLocator("#preview").locator("body").textContent(), /CAFÉ & BOOKS/);
  const sharedExample = JSON.parse(await readFile(resolve(root, "demo/catalog.json"), "utf8"));
  assert.equal(await page.locator("#output").textContent(), sharedExample.expected);

  const measurements = await page.evaluate(async () => {
    const { BrowserEngine } = await import("../pkg/dtl_browser.js");
    const cases = await (await fetch("../tests/parity.json")).json();
    cases.push(await (await fetch("./catalog.json")).json());
    for (const test of cases) {
      const engine = new BrowserEngine();
      try {
        for (const [name, source] of Object.entries(test.templates ?? {})) engine.addTemplate(name, source);
        engine.addTemplate(test.name, test.source);
        if (engine.render(test.name, JSON.stringify(test.context)) !== test.expected) throw new Error(`Mismatch: ${test.name}`);
      } finally { engine.free(); }
    }
    const engine = new BrowserEngine();
    try {
      const source = '<ul>{% for row in rows %}<li>{{ row.name }} / {{ row.category.name }}{% if row.available %} available{% endif %}</li>{% endfor %}</ul>';
      const context = JSON.stringify({ rows: Array.from({ length: 100 }, (_, n) => ({ name: `Product ${n} & <details>`, category: { name: "Books" }, available: n % 3 !== 0 })) });
      engine.addTemplate("benchmark", source);
      for (let i = 0; i < 100; i++) engine.render("benchmark", context);
      const samplesUs = [];
      for (let sample = 0; sample < 15; sample++) {
        const start = performance.now();
        for (let i = 0; i < 200; i++) engine.render("benchmark", context);
        samplesUs.push((performance.now() - start) * 1000 / 200);
      }
      const sorted = [...samplesUs].sort((a, b) => a - b);
      return { parityCases: cases.length, browser: navigator.userAgent, loop100MedianUs: sorted[7], samplesUs };
    } finally { engine.free(); }
  });
  await page.locator("#template").fill("{% load unsupported %}");
  await page.getByRole("button", { name: "Render template", exact: true }).click();
  assert.match(await page.locator("#status").textContent(), /Unsupported tag/);
  await page.locator("#template").fill("Hello {{ title }}");
  await page.getByRole("button", { name: "Render template", exact: true }).click();
  assert.match(await page.locator("#output").textContent(), /Hello Books &amp; browser experiments/);
  await page.locator("#context").fill("broken JSON");
  await page.getByRole("button", { name: "Render template", exact: true }).click();
  assert.equal(await page.locator("#output").textContent(), "");
  assert.deepEqual(errors, []);
  const artifacts = resolve(root, "../target/dtl-browser");
  await mkdir(artifacts, { recursive: true });
  await writeFile(resolve(artifacts, "browser-results.json"), JSON.stringify(measurements, null, 2));
  console.log(JSON.stringify(measurements));
} finally {
  await browser?.close();
  await new Promise((done) => server.close(done));
}
