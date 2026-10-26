import { test } from 'node:test';
import assert from 'node:assert/strict';
import { loadPkg } from './pkg.mjs';

const client = process.env.KERNEL_PKG !== undefined;

test('the client build keeps what a browser host runs', { skip: !client }, async () => {
  const pkg = await loadPkg();
  for (const name of ['Kernel', 'Server', 'installLanguage', 'httpDriverStart', 'httpDriverResume', 'oauthAuthorizePlan', 'osGlobMatches', 'tabUrlMatches', 'evaluateMany', 'scopeRows']) {
    assert.notEqual(pkg[name], undefined, `${name} is missing from the client build`);
  }
});
