#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

cargo build --locked --release -p rtce-wasm --target wasm32-unknown-unknown
mkdir -p web/src/wasm
wasm-bindgen \
  target/wasm32-unknown-unknown/release/rtce_wasm.wasm \
  --out-dir web/src/wasm \
  --out-name rtce_wasm \
  --target web
