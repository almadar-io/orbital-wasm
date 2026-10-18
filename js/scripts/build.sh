#!/bin/sh
# Builds the wasm32 kernel and runs wasm-bindgen into ./pkg (the full kernel: Node hosts and tools) and ./pkg-client
# (the client build: a browser host, without the server-role drivers and the verifier exports).
# Run from orbital-wasm/js. `sh scripts/build.sh full` or `sh scripts/build.sh client` builds one; no argument builds both.
set -eu
cd "$(dirname "$0")/.."
CARGO="${CARGO:-cargo}"

build() {
  variant="$1"
  if [ "$variant" = client ]; then
    OUT=pkg-client
    FEATURES="--no-default-features"
  else
    OUT=pkg
    FEATURES=""
  fi
  # Inside the orbital-rust workspace the kernel builds with its wasm-release profile; a standalone
  # clone of this repo is its own root, so the same settings are passed to the release profile.
  if [ -f ../../../Cargo.toml ] && grep -q '"crates/orbital-wasm"' ../../../Cargo.toml; then
    ROOT="$(cd ../../.. && pwd)"
    TARGET="$ROOT/target"
    PROFILE=wasm-release
    $CARGO build --manifest-path "$ROOT/Cargo.toml" -p orbital-wasm $FEATURES --target wasm32-unknown-unknown --profile "$PROFILE"
  else
    ROOT="$(cd .. && pwd)"
    TARGET="$ROOT/target"
    PROFILE=release
    CARGO_PROFILE_RELEASE_OPT_LEVEL=3 CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
      CARGO_PROFILE_RELEASE_STRIP=debuginfo \
      $CARGO build --manifest-path "$ROOT/Cargo.toml" $FEATURES --target wasm32-unknown-unknown --release
  fi
  wasm-bindgen "$TARGET/wasm32-unknown-unknown/$PROFILE/orbital_wasm.wasm" \
    --target web --out-dir "$OUT" --out-name orbital_wasm --remove-name-section --remove-producers-section
  if command -v wasm-opt >/dev/null 2>&1; then
    wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals --enable-multivalue --enable-reference-types "$OUT/orbital_wasm_bg.wasm" -o "$OUT/orbital_wasm_bg.wasm"
  else
    echo "wasm-opt (binaryen) not found: skipping the -O3 pass; install it with 'brew install binaryen'" >&2
  fi
}

case "${1:-both}" in
  full) build full ;;
  client) build client ;;
  both) build full; build client ;;
  *) echo "usage: build.sh [full|client]" >&2; exit 2 ;;
esac
