import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { pkgDir, loadPkg, languageBundle } from './pkg.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const embed = join(here, '../../../orbital-core/tests/fixtures/embed');
const readJson = (path) => JSON.parse(readFileSync(path, 'utf8'));

const { Kernel, initSync, installLanguage, evaluateGuards, evaluateMany, evaluateTraced, mapListenPayload, scopeRows, checkMutationAccess, resolveRowBinding } = await loadPkg();
initSync({ module: readFileSync(join(pkgDir, 'orbital_wasm_bg.wasm')) });
installLanguage(languageBundle());

function run(script) {
  const schema = readFileSync(join(embed, 'resolved', `${script.schema}.json`), 'utf8');
  const kernel = new Kernel(schema, script.clientId);
  let lastError = '';
  const attempt = (call) => {
    try {
      return call();
    } catch (e) {
      lastError = e.message;
      return { error: e.message };
    }
  };
  const note = (call) => {
    try {
      call();
    } catch (e) {
      lastError = e.message;
    }
    return null;
  };
  const json = (text) => JSON.parse(text);
  return script.steps.map((s) => {
    const entity = s.entityId ?? undefined;
    const text = (v) => JSON.stringify(v);
    switch (s.op) {
      case 'dispatch':
        return attempt(() => json(kernel.dispatch(s.trait, entity, s.event, s.payload === undefined ? undefined : text(s.payload))));
      case 'apply':
        return attempt(() => json(kernel.applyServerResponse(text(s.response), text(s.alreadyDelivered))));
      case 'restore':
        return attempt(() => json(kernel.restore(s.trait, entity, text(s.snapshot))));
      case 'abandon':
        return null;
      case 'tick':
        return attempt(() => json(kernel.tick(s.trait, s.tick, entity)));
      case 'frame': {
        const out = attempt(() => kernel.frame(s.nowMs));
        if (out === undefined) return null;
        if (typeof out === 'string') {
          const frame = json(out);
          if (frame.errors.length > 0) lastError = frame.errors[0];
          return frame;
        }
        return out;
      }
      case 'framePause':
        kernel.framePause();
        return null;
      case 'frameResume':
        kernel.frameResume();
        return null;
      case 'tickCount':
        return kernel.tickCount();
      case 'getState':
        return kernel.getState(s.trait, entity);
      case 'snapshot':
        return json(kernel.snapshot());
      case 'setThemeMode':
        return note(() => kernel.setThemeMode(s.mode));
      case 'setUser':
        return note(() => kernel.setCurrentUser(text(s.user)));
      case 'seed':
        return note(() => kernel.seed(s.entityType, text(s.rows)));
      case 'remount':
        return note(() => kernel.remount(text(s.traits)));
      case 'mount':
        return note(() => kernel.mount(text(s.traits)));
      case 'lastError':
        return lastError;
      default:
        throw new Error(`unknown op ${s.op}`);
    }
  });
}

for (const name of ['basic-entity', 'tick-counter', 'entity-rerender', 'rollback', 'two-orbital-chore', 'two-orbital-task']) {
  test(`${name} matches the C-ABI golden`, () => {
    const script = readJson(join(embed, 'scripts', `${name}.json`));
    const golden = readJson(join(embed, 'golden', `${name}.json`));
    const got = run(script);
    assert.equal(got.length, golden.length);
    got.forEach((out, i) => assert.deepEqual(out, golden[i], `${name} step ${i} ${JSON.stringify(script.steps[i])}`));
  });
}

test('control: a changed step breaks the golden', () => {
  const script = readJson(join(embed, 'scripts', 'rollback.json'));
  const golden = readJson(join(embed, 'golden', 'rollback.json'));
  script.steps[2].event = 'INIT';
  assert.notDeepEqual(run(script), golden);
});

test('a failed call throws an Error carrying the last_error message', () => {
  const schema = readFileSync(join(embed, 'resolved', 'basic-entity.json'), 'utf8');
  const kernel = new Kernel(schema, 'c');
  assert.throws(() => kernel.dispatch('NoSuchTrait', undefined, 'INIT', undefined), (e) => {
    assert.ok(e instanceof Error);
    assert.equal(kernel.lastError(), e.message);
    return true;
  });
  assert.throws(() => kernel.setCurrentUser('{not json'), /parse user_json/);
});

test('an empty schema is refused at construction', () => {
  assert.throws(() => new Kernel('{"name":"Empty","orbitals":[]}', 'c'), /resolved schema declares no orbitals/);
});

test('evaluateMany binds a render in one crossing; a failing expression is its own outcome (EXEC-EVAL-7)', () => {
  const context = JSON.stringify({ entity: { qty: 3 }, payload: { step: 2 }, user: { role: 'admin' }, state: 'open' });
  const out = JSON.parse(evaluateMany(context, JSON.stringify([['+', '@entity.qty', '@payload.step'], ['=', '@user.role', 'admin'], '@state', ['/', 1, 0], ['str/upper', '@payload.missing']])));
  assert.deepEqual(out.slice(0, 4), [{ kind: 'value', value: 5 }, { kind: 'value', value: true }, { kind: 'value', value: 'open' }, { kind: 'value', value: null }]);
  assert.equal(out[4].kind, 'error');
  assert.match(out[4].message, /expected string, got null/);
});

test('evaluateGuards takes booleans only: a list or number is a type error, never read by truthiness (EXEC-EVAL-7)', () => {
  const verdicts = JSON.parse(evaluateGuards(JSON.stringify({ entity: { tags: [], n: 2 } }), JSON.stringify(['@entity.tags', '@entity.n', ['>', '@entity.n', 5], ['>', '@entity.n', 1]])));
  assert.deepEqual(verdicts.map((v) => v.kind), ['error', 'error', 'value', 'value']);
  assert.match(verdicts[0].message, /expected boolean, got array/);
  assert.match(verdicts[1].message, /expected boolean, got number/);
  assert.deepEqual(verdicts.slice(2).map((v) => v.value), [false, true]);
});

test('evaluateTraced records the short-circuited argument as skipped', () => {
  const run = JSON.parse(evaluateTraced('{}', JSON.stringify(['and', ['>', 0, 1], ['=', 1, 1]])));
  assert.deepEqual(run.outcome, { kind: 'value', value: false });
  assert.deepEqual(run.trace.filter((step) => step.kind === 'skip').map((step) => step.path), [[2]]);
});

test('mapListenPayload, scopeRows, checkMutationAccess and resolveRowBinding are the kernel access owner', () => {
  assert.deepEqual(JSON.parse(mapListenPayload(JSON.stringify({ rowId: '@payload.id' }), JSON.stringify({ id: 'r1', extra: 1 }))), { rowId: 'r1' });
  const rows = [{ id: 'a', owner: 'u1' }, { id: 'b', owner: 'u2' }];
  const owner = JSON.stringify(['=', '@entity.owner', '@user.id']);
  const kept = JSON.parse(scopeRows(JSON.stringify({ user: { id: 'u1' } }), JSON.stringify(rows), owner, '', 'Ticket'));
  assert.deepEqual(kept.map((r) => r.id), ['a']);
  assert.equal(JSON.parse(checkMutationAccess('update', JSON.stringify({ user: { id: 'u2' } }), JSON.stringify(rows[0]), owner, 'Ticket')).allowed, false);
  assert.deepEqual(JSON.parse(resolveRowBinding('@user.id', JSON.stringify({ user: { id: 'u1' } }))), { kind: 'value', value: 'u1' });
});
