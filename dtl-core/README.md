# Shared portable template core

An experimental parser and renderer over JSON data, extracted from the browser
prototype. The native Python `PortableEngine` and Wasm `BrowserEngine` adapters
both use this crate. It depends on the existing `dtl-lexer` but has no PyO3,
JavaScript or Django dependencies.

The existing production Python `Engine` / `RustyTemplates` backend remains
separate and retains its full Django compatibility. This extraction does not
route existing templates through a smaller feature set or remove fallback paths.
See [the browser README](../dtl-browser/README.md) for the portable subset,
limitations, shared-template bundle, build steps and verification commands.

Inheritance is resolved through an in-memory registry. Compiled templates retain
named blocks; render-time block stacks select the most-derived body and lazily
resolve `block.super` against its parent. Includes build their own block stack,
while relative names use the origin of the block's defining template. Parent
selection remains dynamic, and registry replacements apply to later renders.

Sharing template source does not require applications to adopt `PortableEngine`:
the exact same demo template bundle renders on the normal Django/Rusty server
backends and in browser Wasm. The extra Python adapter proves the portable core
itself can be shared across both hosts without changing the production backend.
