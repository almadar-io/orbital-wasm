import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { Kernel, initSync } from '../pkg/orbital_wasm.js';

const here = dirname(fileURLToPath(import.meta.url));
const embed = join(here, '../../../orbital-core/tests/fixtures/embed');
const readJson = (path) => JSON.parse(readFileSync(path, 'utf8'));

initSync({ module: readFileSync(join(here, '../pkg/orbital_wasm_bg.wasm')) });

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
