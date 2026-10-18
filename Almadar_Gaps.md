<!-- Gap ledger for this repo: the source of truth for its open gaps. -->
# orbital-wasm — open gaps

Every open gap this repo owns lives here. This file is the source of truth; the monorepo's `docs/Almadar_Gaps.md` only rolls it up.

- **One entry per gap:** `- **<code>** — <what is wrong and where>. <owning package> [mechanical|architectural] — <evidence, prevention rung>`.
- **Codes:** new gaps use this repo's prefix `G-ORBITAL-WASM-`. Take the "Next code" below, then bump it in the same edit. Codes are never reused or renamed.
- **Close by deleting.** Remove the entry in the same commit as the fix.

- **G-ORBITAL-WASM-003** — The browser loaders still load the full kernel: `orbital-shell-chrome` `src/backend.rs` (the generated `kernel.ts` imports `@almadar/kernel` and copies `node_modules/@almadar/kernel/pkg/orbital_wasm_bg.wasm`) must import `@almadar/kernel/client` and copy `pkg-client/orbital_wasm_bg.wasm` (-1.26 MB); `orbital-shell-typescript` `tests/kernel_host.rs` should assert the loader names `@almadar/kernel/client`. orbital-shell-chrome, orbital-shell-typescript [mechanical] — the files were mid-edit by another session when the client build landed; prevention rung: the shell's loader unit test.
- **G-ORBITAL-WASM-004** — The client build still carries `http` signing (aws-sigv4, VAPID, service-account JWT) with `rsa`/`p256`/`num-bigint-dig`, about 0.3 MB; leaving them out would fail a program that declares those auth schemes in a browser host. orbital-core-runtime `http_request` [owner-decision] — measured with `scratchpad/kernel-wasm-split.md` symbol sizes; no language rung applies, it is a capability call.

- **G-ORBITAL-WASM-005** — `StoredServer::from_env_native` (`src/stored.rs`) builds its host credentials with `credentials_from_env()` and no encrypted store, so `ALMADAR_CREDENTIALS_SOURCE=store` neither resolves from nor seeds a store on that path; `orb serve` (`orbital-server` `runtime.rs`, `seed_store_from_env`) and the JS host (`credentialSeedPlan` export, `seedCredentialStore`) do. orbital-wasm [architectural] — prevention rung: none (a missing host port, not a program defect).

Next code: `G-ORBITAL-WASM-006`

