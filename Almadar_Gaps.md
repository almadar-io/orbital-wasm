<!-- Gap ledger for this repo: the source of truth for its open gaps. -->
# orbital-wasm — open gaps

Every open gap this repo owns lives here. This file is the source of truth; the monorepo's `docs/Almadar_Gaps.md` only rolls it up.

- **One entry per gap:** `- **<code>** — <what is wrong and where>. <owning package> [mechanical|architectural] — <evidence, prevention rung>`.
- **Codes:** new gaps use this repo's prefix `G-ORBITAL-WASM-`. Take the "Next code" below, then bump it in the same edit. Codes are never reused or renamed.
- **Close by deleting.** Remove the entry in the same commit as the fix.


Next code: `G-ORBITAL-WASM-008`

- **G-ORBITAL-WASM-006** [mechanical] — The full wasm build still enables `wire-drivers` (Postgres and Redis over raw sockets, Cargo.toml:5-9), retired with the std drivers (R2 shell survey 2026-10-29); credentials.rs (575) still carries OAuth/ticket plumbing (:197-219, lib.rs:560-591) the kernel no longer asks for. Remove. Root cause: T2.

