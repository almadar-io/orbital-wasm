import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { loadPkg, pkgDir, languageBundle } from './pkg.mjs';

const here = dirname(fileURLToPath(import.meta.url));

const client = process.env.KERNEL_PKG !== undefined;

test('the client build keeps what a browser host runs', { skip: !client }, async () => {
  const pkg = await loadPkg();
  for (const name of ['Kernel', 'Server', 'installLanguage', 'osGlobMatches', 'tabUrlMatches', 'parseEventRequest', 'evaluateMany', 'scopeRows']) {
    assert.notEqual(pkg[name], undefined, `${name} is missing from the client build`);
  }
});

test('a kernel refuses to start, with the typed error, until the language is installed, then starts', async () => {
  const pkg = await loadPkg();
  pkg.initSync({ module: readFileSync(join(pkgDir, 'orbital_wasm_bg.wasm')) });
  const schema = readFileSync(join(here, 'fixtures/notes.resolved.json'), 'utf8');
  assert.throws(() => new pkg.Kernel(schema, 'c1'), /ORB_LANGUAGE_DATA_NOT_INSTALLED/);
  assert.throws(() => new pkg.Server(schema), /ORB_LANGUAGE_DATA_NOT_INSTALLED/);
  pkg.installLanguage(languageBundle());
  assert.doesNotThrow(() => new pkg.Kernel(schema, 'c1'));
  assert.doesNotThrow(() => new pkg.Server(schema));
});
