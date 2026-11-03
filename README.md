<div align="center">

# orbital-wasm

**The Almadar kernel as WebAssembly for JavaScript hosts, published to npm as `@almadar/kernel`.**

[![CI](https://github.com/almadar-io/orbital-wasm/actions/workflows/ci.yml/badge.svg)](https://github.com/almadar-io/orbital-wasm/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/@almadar/kernel)](https://www.npmjs.com/package/@almadar/kernel)
[![License: BUSL-1.1](https://img.shields.io/badge/license-BUSL--1.1-blue)](LICENSE)

</div>

`wasm-bindgen` marshalling over `orbital_core::embed`. It exposes `Kernel` (the client role) and `Server` (the server role, whose I/O the host answers), both speaking the kernel's JSON wire. A failed operation throws a JS `Error`. The same kernel runs natively through [orbital-ffi](https://github.com/almadar-io/orbital-ffi) on iOS and Android.

## Where this fits

```mermaid
flowchart TB
  subgraph kernel["Kernel and substrate"]
    core["orbital-core<br/>IR · evaluator · kernel"]
    canvas["orbital-canvas<br/>2D/3D render primitives"]
  end
  subgraph lang["Language"]
    lolo["orbital-lolo<br/>.lolo → .orb"]
    compiler["orbital-compiler<br/>validate · codegen"]
  end
  subgraph hosts["Shells and hosts"]
    ts["orbital-shell-typescript<br/>web · node · @almadar/core"]
    rs["orbital-shell-rust<br/>desktop · tui"]
    py["orbital-shell-python"]
    ios["orbital-shell-ios"]
    android["orbital-shell-android"]
    os["orbital-shell-os"]
    wasm["orbital-wasm<br/>browser kernel"]
    ffi["orbital-ffi<br/>C ABI for mobile"]
    router["orbital-router"]
  end
  cli["orbital-cli<br/>the orb binary"]
  ws["orbital-rust<br/>workspace · cross-crate tests"]
  dist["orb<br/>npm · releases"]
  subgraph pkgs["Language packages"]
    std["almadar-std<br/>@almadar/std substrate"]
    beh["behaviors<br/>themed organisms"]
    langs["lolo-ar · lolo-sl<br/>language packs"]
  end
  subgraph apps["Apps and sites"]
    builder["builder (studio)"]
    kflow["kflow-academy"]
    nata["nata"]
    rawi["rawi"]
    site["almadar (websites)"]
  end
  core --> lolo & compiler & ts & rs & py & ios & android & os & wasm & ffi & router
  canvas --> ts & rs & ffi
  lolo --> compiler
  compiler --> cli
  ts & rs & py & ios & android & os --> cli
  cli --> dist
  ws -.pins.-> core & lolo & compiler & cli
  dist --> std
  std --> beh
  std --> langs
  beh --> builder & kflow & nata & rawi & site
  click core "https://github.com/almadar-io/orbital-core"
  click canvas "https://github.com/almadar-io/orbital-canvas"
  click lolo "https://github.com/almadar-io/orbital-lolo"
  click compiler "https://github.com/almadar-io/orbital-compiler"
  click ts "https://github.com/almadar-io/orbital-shell-typescript"
  click rs "https://github.com/almadar-io/orbital-shell-rust"
  click py "https://github.com/almadar-io/orbital-shell-python"
  click ios "https://github.com/almadar-io/orbital-shell-ios"
  click android "https://github.com/almadar-io/orbital-shell-android"
  click os "https://github.com/almadar-io/orbital-shell-os"
  click wasm "https://github.com/almadar-io/orbital-wasm"
  click ffi "https://github.com/almadar-io/orbital-ffi"
  click router "https://github.com/almadar-io/orbital-router"
  click cli "https://github.com/almadar-io/orbital-cli"
  click ws "https://github.com/almadar-io/orbital-rust"
  click dist "https://github.com/almadar-io/orb"
  click std "https://github.com/almadar-io/almadar-std"
  click beh "https://github.com/almadar-io/behaviors"
  click langs "https://github.com/almadar-io/lolo-ar"
  click builder "https://github.com/itechdom/almadar-studio"
  click kflow "https://github.com/almadar-io/almadar-kflow-academy"
  click nata "https://github.com/almadar-io/almadar-nata"
  click rawi "https://github.com/almadar-io/almadar-rawi"
  click site "https://github.com/almadar-io/almadar"
  classDef here fill:#2563eb,color:#fff,stroke:#1e40af,stroke-width:3px
  class wasm here
```

| Upstream (this repo depends on) | Downstream (depends on this repo) |
|---|---|
| [orbital-core](https://github.com/almadar-io/orbital-core): the embedded kernel it wraps | Browser and Node hosts that load `@almadar/kernel`. The workspace `make install-dev` runs `scripts/build.sh client` here before building the served web client |

## Quick start

Use the package from npm:

```bash
npm install @almadar/kernel
```

The package has two entry points: `@almadar/kernel` (the full kernel, for Node hosts and tools) and `@almadar/kernel/client` (the browser build, without the raw-socket store drivers). The `.wasm` files are exported as `./kernel.wasm` and `./client.wasm`. It requires Node 22 or newer.

## What's inside

| Path | Purpose |
|---|---|
| `src/` | The crate: `api.rs` (`KernelApi`, `ServerApi` over JSON strings), `lib.rs` (the `Kernel` and `Server` wasm-bindgen classes), `credentials.rs`, `stored.rs` |
| `js/` | The `@almadar/kernel` npm package; `scripts/build.sh` builds `pkg/` (full) and `pkg-client/` (client) |
| `js/test/` | Node tests for the built package |
| `tests/` | Crate tests that replay orbital-core's embed goldens |

Cargo features: `full` (default, enables `wire-drivers`) and `wire-drivers`. The client build is `--no-default-features`.

## Development

The crate tests read orbital-core's embed fixtures from a sibling checkout at `../orbital-core` (inside the monorepo that is `orbital-rust/crates/orbital-core`).

```bash
git clone https://github.com/almadar-io/orbital-core ../orbital-core
npm install --no-save --no-package-lock @almadar/core@^10 @almadar/std@^16 @almadar/lolo-ar@^1 @almadar/lolo-sl@^1
cargo test
rustup target add wasm32-unknown-unknown && cargo install wasm-bindgen-cli --version 0.2.126 --locked
cd js && sh scripts/build.sh && npm test
```

- CI (`.github/workflows/ci.yml`) runs the same steps on push and pull request to `main`, with binaryen version_133 for `wasm-opt`.
- Pushing a `js-v*` tag runs `publish-js.yml`, which builds and publishes `@almadar/kernel` under the `next` dist-tag; the orb release smoke promotes it to `latest`.
- Open gaps are tracked in [`Almadar_Gaps.md`](Almadar_Gaps.md). Work on `main` only; there are no feature branches.
- Inside the monorepo this repo is the submodule `orbital-rust/crates/orbital-wasm`.

## Docs

- [Orbital Core](https://github.com/itechdom/kflow.ai/blob/main/docs/Almadar_Orbital_Core.md)
- [Orbital Shell TypeScript](https://github.com/itechdom/kflow.ai/blob/main/docs/Almadar_Orbital_Shell_TypeScript.md)
- [Execution Semantics](https://github.com/itechdom/kflow.ai/blob/main/docs/Almadar_Execution_Semantics.md)

## License

[Business Source License 1.1](LICENSE): free for non-commercial use and internal evaluation; commercial production use needs a licence from Almadar FZE. Converts to Apache-2.0 on 2030-02-01.
