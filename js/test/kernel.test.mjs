import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, join } from 'node:path';
import { pkgDir, loadPkg, languageBundle } from './pkg.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const embed = join(here, '../../../orbital-core/tests/fixtures/embed');
const readJson = (path) => JSON.parse(readFileSync(path, 'utf8'));
const utf8 = new TextEncoder();

const { Kernel, initSync, installLanguage, evaluateGuards, evaluateMany, evaluateTraced, mapListenPayload, scopeRows, checkMutationAccess, resolveRowBinding } = await loadPkg();
initSync({ module: readFileSync(join(pkgDir, 'orbital_wasm_bg.wasm')) });
installLanguage(languageBundle());

function run(script) {
  const schema = readFileSync(join(embed, 'resolved', `${script.schema}.json`), 'utf8');
  const kernel = new Kernel(utf8.encode(schema), script.clientId);
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
  const held = new Map();
  const json = (text) => resolveHeld(JSON.parse(text), held);
  return script.steps.map((s) => {
    const answer = step(s);
    for (const id of JSON.parse(kernel.heldReleased())) held.delete(id);
    return answer;
  });
  function step(s) {
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
  }
}

/** The answer as the host reads it: each held leaf replaced by the value it names (EXEC-DIST-16). */
function resolveHeld(v, held) {
  if (Array.isArray(v)) return v.map((x) => resolveHeld(x, held));
  if (v === null || typeof v !== 'object') return v;
  if (typeof v.$held === 'number') {
    if (v.value === undefined) {
      assert.ok(held.has(v.$held), `held value ${v.$held} was handed to this host and not released`);
      return held.get(v.$held);
    }
    const full = resolveHeld(v.value, held);
    held.set(v.$held, full);
    return full;
  }
  return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, resolveHeld(x, held)]));
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
  const kernel = new Kernel(utf8.encode(schema), 'c');
  assert.throws(() => kernel.dispatch('NoSuchTrait', undefined, 'INIT', undefined), (e) => {
    assert.ok(e instanceof Error);
    assert.equal(kernel.lastError(), e.message);
    return true;
  });
  assert.throws(() => kernel.seed('Task', '{not json'), /parse rows_json/);
});

test('an empty schema is refused at construction', () => {
  assert.throws(() => new Kernel(utf8.encode('{"name":"Empty","orbitals":[]}'), 'c'), /resolved schema declares no orbitals/);
});

test('evaluateMany binds a render in one crossing; a failing expression is its own outcome (EXEC-EVAL-7)', () => {
  const context = JSON.stringify({ entity: { qty: 3 }, payload: { step: 2 }, user: { role: 'admin' }, state: 'open' });
  const out = JSON.parse(evaluateMany(context, JSON.stringify([['+', '@entity.qty', '@payload.step'], ['=', '@user.role', 'admin'], '@state', ['/', 1, 0], ['str/upper', '@payload.missing']])));
  assert.deepEqual(out.slice(0, 4), [{ kind: 'value', value: 5 }, { kind: 'value', value: true }, { kind: 'value', value: 'open' }, { kind: 'value', value: null }]);
  assert.equal(out[4].kind, 'error');
  assert.match(out[4].message, /expected string, got null/);
});

test('evaluateGuards takes booleans only: a list or number is a type error, never read by truthiness (EXEC-EVAL-7)', () => {
  const verdicts = JSON.parse(evaluateGuards(JSON.stringify({ entity: { tags: [], n: 2, x: 2.5 } }), JSON.stringify(['@entity.tags', '@entity.n', '@entity.x', ['>', '@entity.n', 5], ['>', '@entity.n', 1]])));
  assert.deepEqual(verdicts.map((v) => v.kind), ['error', 'error', 'error', 'value', 'value']);
  assert.match(verdicts[0].message, /expected boolean, got array/);
  assert.match(verdicts[1].message, /expected boolean, got int/);
  assert.match(verdicts[2].message, /expected boolean, got number/);
  assert.deepEqual(verdicts.slice(3).map((v) => v.value), [false, true]);
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

const loadInstanceFactory = () => import(pathToFileURL(join(pkgDir, 'orbital_wasm_instance.js')).href);
const compiled = () => new WebAssembly.Module(readFileSync(join(pkgDir, 'orbital_wasm_bg.wasm')));
const notesSchema = () => utf8.encode(readFileSync(join(here, 'fixtures/notes.resolved.json'), 'utf8'));

test('each kernel instance has its own memory, so one growing leaves the other as it was', async () => {
  const { createKernelInstance } = await loadInstanceFactory();
  const module = compiled();
  const a = await createKernelInstance(module, languageBundle());
  const b = await createKernelInstance(module, languageBundle());
  assert.notEqual(a.memory, b.memory, 'two instances, two memories');
  const before = a.memory.buffer.byteLength;
  const big = JSON.parse(readFileSync(join(here, 'fixtures/notes.resolved.json'), 'utf8'));
  big.description = 'x'.repeat(32 * 1024 * 1024);
  const kernels = [new b.Kernel(utf8.encode(JSON.stringify(big)), 'client-b')];
  assert.ok(b.memory.buffer.byteLength > before, 'the second instance grew');
  assert.equal(a.memory.buffer.byteLength, before, 'the first did not');
  assert.equal(typeof new a.Kernel(notesSchema(), 'client-a').snapshot(), 'string', 'each instance runs its own kernels');
  kernels.forEach((k) => k.free());
});

test('an instance nothing references any more has its memory collected', async () => {
  assert.equal(typeof globalThis.gc, 'function', 'the suite runs with --expose-gc');
  const { createKernelInstance } = await loadInstanceFactory();
  const module = compiled();
  const held = await (async () => {
    const instance = await createKernelInstance(module, languageBundle());
    const kernel = new instance.Kernel(notesSchema(), 'client-c');
    kernel.free();
    return new WeakRef(instance.memory);
  })();
  for (let i = 0; i < 20 && held.deref() !== undefined; i++) {
    await new Promise((r) => setTimeout(r, 10));
    globalThis.gc();
  }
  assert.equal(held.deref(), undefined, 'the dropped instance released its memory');
});

/** Runs `body` with `new WebAssembly.Instance` refused, as a browser's main thread refuses it for a module over 8 MB. */
async function withoutSyncInstantiation(body) {
  const Sync = WebAssembly.Instance;
  WebAssembly.Instance = new Proxy(Sync, {
    construct() {
      throw new RangeError('WebAssembly.Instance is disallowed on the main thread, if the buffer size is larger than 8MB');
    },
  });
  try {
    return await body();
  } finally {
    WebAssembly.Instance = Sync;
  }
}

test('an instance is made without synchronous instantiation, which a browser main thread refuses for the kernel', async () => {
  const { createKernelInstance } = await loadInstanceFactory();
  const module = compiled();
  const instance = await withoutSyncInstantiation(() => createKernelInstance(module, languageBundle()));
  assert.equal(typeof new instance.Kernel(notesSchema(), 'client-e').snapshot(), 'string');
});

test('control: the refusal stands in for the browser, so a synchronous instantiation fails under it', async () => {
  const module = compiled();
  await assert.rejects(withoutSyncInstantiation(() => new WebAssembly.Instance(module, {})), RangeError);
});

test('control: the module-level kernel keeps working beside the instances', () => {
  assert.equal(typeof new Kernel(notesSchema(), 'client-d').snapshot(), 'string');
});

test('control: an instance still referenced keeps its memory through the same collections', async () => {
  const { createKernelInstance } = await loadInstanceFactory();
  const instance = await createKernelInstance(compiled(), languageBundle());
  const held = new WeakRef(instance.memory);
  for (let i = 0; i < 20; i++) {
    await new Promise((r) => setTimeout(r, 10));
    globalThis.gc();
  }
  assert.equal(held.deref(), instance.memory);
});

// EXEC-DIST-16b: twin of `wasm_api_session` `a_folded_blob_is_stored_and_rendered_by_its_marker_and_released_when_replaced`.
const blobViewer = JSON.stringify({
  name: 'viewer', typeContractVersion: 1, orbitals: [{
    name: 'Sessions',
    entity: { name: 'Session', persistence: 'runtime', fields: [{ name: 'id', type: 'string' }] },
    traits: [{ name: 'GuestSession', linkedEntity: 'Session',
      emits: [{ event: 'GUEST_CLIENT', scope: 'external', payloadSchema: [{ name: 'guest', type: 'string', required: true }, { name: 'program', type: 'SExpr', required: true }] }],
      stateMachine: { states: [{ name: 'idle', isInitial: true }], transitions: [{ from: 'idle', to: 'idle', event: 'LOAD', effects: [] }] } }],
  }, {
    name: 'Stages',
    entity: { name: 'Stage', persistence: 'runtime', fields: [{ name: 'id', type: 'string' }, { name: 'client', type: 'SExpr' }] },
    traits: [{ name: 'GuestStage', linkedEntity: 'Stage', dispatch: 'client',
      listens: [{ event: 'GUEST_CLIENT', triggers: 'GUEST_CLIENT', source: { kind: 'orbital', orbital: 'Sessions', trait: 'GuestSession' } }],
      stateMachine: {
        events: [{ key: 'GUEST_CLIENT', name: 'Guest Client', payloadSchema: [{ name: 'guest', type: 'string', required: true }, { name: 'program', type: 'SExpr', required: true }] }],
        states: [{ name: 'idle', isInitial: true }],
        transitions: [{ from: 'idle', to: 'idle', event: 'GUEST_CLIENT', effects: [['set', '@entity.client', '@payload.program'], ['render-ui', 'main', { type: 'guest-view', guest: '@payload.guest', program: '@entity.client' }]] }],
      } }],
  }],
});
const guestClient = (program) => utf8.encode(JSON.stringify({ success: true, transitioned: true, emittedEvents: [{ event: 'GUEST_CLIENT', payload: { guest: 'g', program }, source: { orbital: 'Sessions', trait: 'GuestSession' } }] }));
const renderedProgram = (text) => {
  const held = new Map();
  const answer = resolveHeld(JSON.parse(text), held);
  const render = answer.fold.response.clientEffectsByTrait.find((e) => e.traitName === 'GuestStage' && e.effect[0] === 'render-ui');
  return render.effect[2].program;
};

test('a folded blob is stored and rendered by its marker, and released once replaced (EXEC-DIST-16b)', () => {
  const kernel = new Kernel(utf8.encode(blobViewer), 'c');
  let flight = kernel.openFlight();
  assert.deepEqual(renderedProgram(kernel.foldInOrder(flight, guestClient({ $blob: 5 }), undefined, undefined, '[5]')), { $blob: 5 });
  kernel.endFlight(flight);
  assert.deepEqual(JSON.parse(kernel.blobsReleased()), []);
  flight = kernel.openFlight();
  assert.deepEqual(renderedProgram(kernel.foldInOrder(flight, guestClient({ $blob: 6 }), undefined, undefined, '[6]')), { $blob: 6 });
  kernel.endFlight(flight);
  assert.deepEqual(JSON.parse(kernel.blobsReleased()), [5]);
  assert.throws(() => kernel.foldInOrder(kernel.openFlight(), guestClient({ $blob: 9 }), undefined, undefined, '[7]'), /never handed/);
});

test('control: a fold with no blobs handed reads the program inline', () => {
  const kernel = new Kernel(utf8.encode(blobViewer), 'c');
  const program = { name: 'crm', orbitals: [] };
  assert.deepEqual(renderedProgram(kernel.foldInOrder(kernel.openFlight(), guestClient(program))), program);
});
