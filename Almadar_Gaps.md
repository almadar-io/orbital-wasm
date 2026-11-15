<!-- Gap ledger for this repo: the source of truth for its open gaps. -->
# orbital-wasm — open gaps

Every open gap this repo owns lives here. This file is the source of truth; the monorepo's `docs/Almadar_Gaps.md` only rolls it up.

- **One entry per gap:** `- **<code>** — <what is wrong and where>. <owning package> [mechanical|architectural] — <evidence, prevention rung>`.
- **Codes:** new gaps use this repo's prefix `G-ORBITAL-WASM-`. Take the "Next code" below, then bump it in the same edit. Codes are never reused or renamed.
- **Close by deleting.** Remove the entry in the same commit as the fix.

Next code: `G-ORBITAL-WASM-010`

- **G-ORBITAL-WASM-009** [architectural] — The browser kernel's wasm linear memory only grows: one tab reaches about 540 MB after std-kflow and osama-site are opened once (114 MB on std-accounting alone) and keeps it after both close, though the guest kernels are freed (live kernels stay at 2 after the 2026-11-14 night fix). WebAssembly memory cannot shrink, so every large guest raises the tab's floor for its life. Measured again 2026-11-15 moving orb-medar-home ↔ osama-site four times in one tab: wasm 147 → 370 (first osama-site) → 498 MB (second) and flat after, while the JS heap returns to about 100 MB, the GPU process to about 180 MB and live kernels stay at 2: the owner's "about 400 MB" on the orb site is this high-water. What in osama-site peaks at about 350 MB of linear memory is not attributed yet: the wasm build installs `CountingAlloc::thread_only`, which keeps no live or peak bytes. Fix direction (needs a design): run guest kernels in their own instance (a worker or a separate wasm instance per guest) so closing a guest returns its memory. Reproduce: the catalog sequence in scratchpad `mem-seq.mjs` (wasm buffer byteLength per step). Prevention rung: (3) the verifiers (the per-open memory budget). Severity: low. Root cause: one-off.
