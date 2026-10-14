#!/bin/sh
# Builds the wasm32 kernel and runs wasm-bindgen into ./pkg. Run from orbital-wasm/js.
set -eu
cd "$(dirname "$0")/.."
# Inside the orbital-rust workspace the kernel builds with its wasm-release profile; a standalone
# clone of this repo is its own root, so the same settings are passed to the release profile.
if [ -f ../../../Cargo.toml ] && grep -q '"crates/orbital-wasm"' ../../../Cargo.toml; then
  TARGET=../../../target
  PROFILE=wasm-release
  cargo build --manifest-path ../../../Cargo.toml -p orbital-wasm --target wasm32-unknown-unknown --profile "$PROFILE"
else
  TARGET=../target
  PROFILE=release
  CARGO_PROFILE_RELEASE_OPT_LEVEL=3 CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
    CARGO_PROFILE_RELEASE_STRIP=debuginfo \
    cargo build --manifest-path ../Cargo.toml --target wasm32-unknown-unknown --release
fi
wasm-bindgen "$TARGET/wasm32-unknown-unknown/$PROFILE/orbital_wasm.wasm" \
  --target web --out-dir pkg --out-name orbital_wasm --remove-name-section --remove-producers-section
if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals --enable-multivalue --enable-reference-types pkg/orbital_wasm_bg.wasm -o pkg/orbital_wasm_bg.wasm
else
  echo "wasm-opt (binaryen) not found: skipping the -O3 pass; install it with 'brew install binaryen'" >&2
fi
