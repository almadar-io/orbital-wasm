import { fileURLToPath, pathToFileURL } from 'node:url';
import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

/** The wasm-bindgen output under test: `pkg` (the full kernel), or the directory `KERNEL_PKG` names (the client build, `pkg-client`). */
export const pkgDir = process.env.KERNEL_PKG === undefined ? join(root, 'pkg') : resolve(root, process.env.KERNEL_PKG);

export const loadPkg = () => import(pathToFileURL(join(pkgDir, 'orbital_wasm.js')).href);

/** The installed language as the bundle a browser host hands its kernel (`installLanguage`). */
export const languageBundle = () => {
  const { readLanguageBundle } = createRequire(import.meta.url)('@almadar/core/language-data');
  return JSON.stringify(readLanguageBundle(root));
};
