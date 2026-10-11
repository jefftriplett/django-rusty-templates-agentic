import init, { BrowserEngine } from "../pkg/dtl_browser.js";

const element = (id) => document.getElementById(id);
const status = element("status");
const button = element("render");
let engine;

function render() {
  try {
    const context = element("context").value;
    JSON.parse(context);
    const compileStart = performance.now();
    engine.addTemplate("base.html", element("base").value);
    engine.addTemplate("card.html", element("card").value);
    engine.addTemplate("page.html", element("template").value);
    const compileMs = performance.now() - compileStart;
    const renderStart = performance.now();
    const html = engine.render("page.html", context);
    const renderMs = performance.now() - renderStart;
    element("output").textContent = html;
    element("preview").srcdoc = html;
    status.removeAttribute("data-error");
    status.textContent = `Compiled in ${compileMs.toFixed(3)} ms · Rendered in ${renderMs.toFixed(3)} ms`;
  } catch (error) {
    status.dataset.error = "true";
    status.textContent = String(error);
    element("output").textContent = "";
    element("preview").srcdoc = "";
  }
}

try {
  const response = await fetch("./catalog.json");
  if (!response.ok) throw new Error("Could not load the shared template bundle");
  const example = await response.json();
  element("base").value = example.templates["base.html"];
  element("card").value = example.templates["card.html"];
  element("template").value = example.source;
  element("context").value = JSON.stringify(example.context, null, 2);
  await init();
  engine = new BrowserEngine();
  button.disabled = false;
  button.textContent = "Render template";
  button.addEventListener("click", render);
  render();
} catch (error) {
  status.dataset.error = "true";
  status.textContent = `Could not load WebAssembly: ${error}`;
}
window.addEventListener("pagehide", (event) => {
  if (!event.persisted) engine?.free();
});
