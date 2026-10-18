import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, join, resolve } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

/** The wasm-bindgen output under test: `pkg` (the full kernel), or the directory `KERNEL_PKG` names (the client build, `pkg-client`). */
export const pkgDir = process.env.KERNEL_PKG === undefined ? join(root, 'pkg') : resolve(root, process.env.KERNEL_PKG);

export const loadPkg = () => import(pathToFileURL(join(pkgDir, 'orbital_wasm.js')).href);
