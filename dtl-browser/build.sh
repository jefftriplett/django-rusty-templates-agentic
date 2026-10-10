#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bindgen_version=0.2.129
if ! command -v wasm-bindgen >/dev/null; then
    echo "Install bindings tool: cargo install wasm-bindgen-cli --version $bindgen_version --locked" >&2
    exit 1
fi
if [[ "$(wasm-bindgen --version)" != "wasm-bindgen $bindgen_version" ]]; then
    echo "This lockfile requires wasm-bindgen-cli $bindgen_version." >&2
    exit 1
fi
cargo build --locked -p dtl-browser --target wasm32-unknown-unknown --profile wasm
wasm-bindgen --target web --out-dir dtl-browser/pkg --out-name dtl_browser \
    target/wasm32-unknown-unknown/wasm/dtl_browser.wasm
