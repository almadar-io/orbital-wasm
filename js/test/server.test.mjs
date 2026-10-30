import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { pkgDir, loadPkg, languageBundle } from './pkg.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const { Server, initSync, installLanguage } = await loadPkg();
initSync({ module: readFileSync(join(pkgDir, 'orbital_wasm_bg.wasm')) });
installLanguage(languageBundle());

const fixture = (name) => readFileSync(join(here, 'fixtures', `${name}.resolved.json`), 'utf8');
const schema = fixture('notes');

const answerOf = (request, rows, refuse) => {
  switch (request.kind) {
    case 'createRow':
      if (refuse) return { kind: 'row', result: { error: 'disk full' } };
      rows.push(request.data);
      return { kind: 'row', result: { ok: request.data } };
    case 'listRows':
      return { kind: 'rows', result: { ok: rows } };
    case 'readRow':
      return { kind: 'row', result: { ok: rows.find((row) => row.id === request.id) ?? null } };
    default:
      throw new Error(`unexpected request ${request.kind}`);
  }
};

async function drive(server, orbital, request, rows, refuse = false) {
  let progress = JSON.parse(server.start(orbital, JSON.stringify(request)));
  while (progress.status === 'await') {
    const replies = await Promise.all(
      progress.requests.map(async (p) => ({ id: p.id, answer: answerOf(p.request, rows, refuse) })),
    );
    progress = JSON.parse(server.resume(progress.flight, JSON.stringify(replies)));
  }
  return progress.response;
}

test('a dispatch runs to its response through the host answers', async () => {
  const server = new Server(schema);
  const rows = [];
  const response = await drive(server, 'Main', { event: 'SAVE', targetTrait: 'Draft' }, rows);
  assert.equal(response.states.Draft, 'saved');
  assert.deepEqual(rows, [{ id: 'd1', title: 'draft' }]);
  assert.equal(server.getState('Draft'), 'saved');
  assert.equal(server.inFlight(), 0);
});

test('control: a refused write takes the declared failure path', async () => {
  const server = new Server(schema);
  const rows = [];
  const response = await drive(server, 'Main', { event: 'SAVE', targetTrait: 'Draft' }, rows, true);
  assert.equal(response.states.Draft, 'failed');
  assert.equal(response.effectResults[0].error, 'disk full');
  assert.deepEqual(rows, []);
});

test('two servers interleave their awaits without crossing', async () => {
  const [a, b] = [new Server(schema), new Server(schema)];
  const request = { event: 'SAVE', targetTrait: 'Draft' };
  const [ra, rb] = await Promise.all([
    drive(a, 'Main', request, []),
    drive(b, 'Main', request, [], true),
  ]);
  assert.equal(ra.states.Draft, 'saved');
  assert.equal(rb.states.Draft, 'failed');
});

test('errors throw and are the last error; flights are validated', () => {
  const server = new Server(schema);
  assert.throws(() => server.start('Nowhere', '{"event":"SAVE"}'), /Orbital not found/);
  assert.match(server.lastError(), /Orbital not found/);
  assert.throws(() => server.resume(0.5, '[]'), /not a flight id/);
  assert.throws(() => server.resume(7, '[]'), /flight 7/);
  assert.throws(() => new Server('{"name":"Empty","orbitals":[]}'), /no orbitals/);
  const started = JSON.parse(server.start('Main', '{"event":"SAVE","targetTrait":"Draft"}'));
  assert.equal(server.inFlight(), 1);
  assert.equal(server.abandon(started.flight), true);
  assert.equal(server.abandon(started.flight), false);
});

const tickSchema = fixture('ticks');

const tick = { event: '$TICK_beat', runTick: { trait: 'Scoped', tick: 'beat' }, targetTrait: 'Scoped', entityId: 'c1' };

test('a tick fires only while a client holds its trait, and the ledger is driven by JSON', async () => {
  const server = new Server(tickSchema);
  const rows = [];
  const paused = await drive(server, 'Board', tick, rows);
  assert.deepEqual(paused.emittedEvents, []);
  server.mountClient('tab-1', 'Board', '["Scoped"]');
  const fired = await drive(server, 'Board', tick, rows);
  assert.deepEqual(fired.emittedEvents.map((e) => e.event), ['PULSE']);
  server.unmountClient('tab-1', 'Board', '["Scoped"]');
  assert.deepEqual((await drive(server, 'Board', tick, rows)).emittedEvents, []);
  server.mountClient('tab-2', 'Board', '["Scoped"]');
  server.releaseClient('tab-2');
  assert.deepEqual((await drive(server, 'Board', tick, rows)).emittedEvents, []);
  assert.throws(() => server.mountClient('tab-1', 'Board', 'not json'), /traits_json/);
});

test('a fired tick names its declaring orbital, and the coalesced relay keeps the newest per key', async () => {
  const server = new Server(tickSchema);
  server.mountClient('tab-1', 'Board', '["Scoped"]');
  assert.deepEqual(JSON.parse(server.tickPass(0)), []);
  assert.deepEqual(JSON.parse(server.tickPass(10)), [{ orbital: 'Board', trait: 'Scoped', tick: 'beat' }]);
  const stamped = (x) => JSON.stringify({ event: 'MOVED', payload: { x }, clientId: 'tab-1', tick: 'beat', sourceTrait: 'Scoped' });
  assert.equal(JSON.parse(server.relayTick('Board', stamped(1))), 50);
  assert.equal(JSON.parse(server.relayTick('Board', stamped(2))), null);
  const items = JSON.parse(server.flushTickRelay());
  assert.equal(items.length, 1);
  assert.deepEqual(items[0].payload, { x: 2 });
  assert.equal(server.declaresTicks(), true);
});

test('a stateful server refuses carried circuit state outside a delegated leg, and an undeclared locale', async () => {
  const server = new Server(tickSchema);
  server.setStateful(true);
  const carried = JSON.parse(server.start('Board', JSON.stringify({ event: 'GO', traits: [{ trait: 'Scoped', from: 'idle' }] })));
  assert.equal(carried.response.success, false);
  assert.equal(carried.response.rejections[0].code, 'client-circuit-state-refused');
  server.setMessages('["en","ar"]', '{}');
  const locale = JSON.parse(server.start('Board', JSON.stringify({ event: 'GO', targetTrait: 'Scoped', locale: 'fr' })));
  assert.equal(locale.response.rejections[0].code, 'locale-not-declared');
});

test('the program messages are set by JSON and picked by the request locale', async () => {
  const server = new Server(tickSchema);
  server.mountClient('tab-1', 'Board', '["Scoped"]');
  server.setMessages('["en","ar"]', JSON.stringify({ en: { k: 'Hello' }, ar: { k: 'مرحبا' } }));
  const ar = await drive(server, 'Board', { ...tick, locale: 'ar' }, []);
  assert.match(JSON.stringify(ar.clientEffectsByTrait), /مرحبا/);
  const en = await drive(server, 'Board', { ...tick, locale: 'en' }, []);
  assert.match(JSON.stringify(en.clientEffectsByTrait), /Hello/);
  assert.throws(() => server.setMessages('{', '{}'), /locales_json/);
});
