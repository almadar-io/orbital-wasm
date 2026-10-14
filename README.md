# orbital-wasm

The embedded Almadar kernel for JavaScript hosts, published to npm as [`@almadar/kernel`](https://www.npmjs.com/package/@almadar/kernel). It is `wasm-bindgen` marshalling over `orbital_core::embed`: `Kernel` (the client role) and `Server` (the server role, whose I/O the host answers), both speaking the kernel's JSON wire.

Part of [Almadar](https://almadar.io)'s Orbital compiler, which turns a `.lolo`/`.orb` program into a full-stack app. The same kernel runs natively through [`orbital-ffi`](https://github.com/almadar-io/orbital-ffi) on iOS and Android.

Depends on [`orbital-core`](https://github.com/almadar-io/orbital-core).

## Layout

- `src/` — the crate.
- `js/` — the `@almadar/kernel` npm package (`scripts/build.sh` builds `pkg/`; `test/` runs it in Node).
- `tests/` — the crate's tests, replaying orbital-core's embed goldens.

## Build and test

The tests read orbital-core's embed fixtures from a sibling checkout, and the crate reads the Almadar language data from the installed npm packages:

```bash
git clone https://github.com/almadar-io/orbital-core ../orbital-core
npm install --no-save --no-package-lock @almadar/core@^10 @almadar/std@^16 @almadar/lolo-ar@^1 @almadar/lolo-sl@^1
cargo test
rustup target add wasm32-unknown-unknown && cargo install wasm-bindgen-cli --version 0.2.126 --locked
cd js && sh scripts/build.sh && npm test
```

## License

[Business Source License 1.1](LICENSE) — free for non-commercial and evaluation use; converts to Apache-2.0 on the change date.
