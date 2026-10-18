import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { pkgDir, loadPkg } from './pkg.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const { Server, initSync } = await loadPkg();
initSync({ module: readFileSync(join(pkgDir, 'orbital_wasm_bg.wasm')) });

const emit = { emit: { success: 'SAVED', failure: 'SAVE_FAILED' } };
const schema = JSON.stringify({
  name: 'notes',
  orbitals: [{
    name: 'Main',
    entity: { name: 'Note', persistence: 'persistent', collection: 'notes', fields: [{ name: 'id', type: 'string' }, { name: 'title', type: 'string' }] },
    traits: [{
      name: 'Draft', linkedEntity: 'Note',
      stateMachine: {
        states: [{ name: 'editing', isInitial: true }, { name: 'saving' }, { name: 'saved' }, { name: 'failed' }],
        transitions: [
          { from: 'editing', to: 'saving', event: 'SAVE', effects: [['persist', 'create', 'Note', { id: 'd1', title: 'draft' }, emit]] },
          { from: 'saving', to: 'saved', event: 'SAVED' },
          { from: 'saving', to: 'failed', event: 'SAVE_FAILED' },
        ],
      },
    }],
  }],
});

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

const tickSchema = JSON.stringify({
  name: 'ticks',
  locales: ['en', 'ar'],
  orbitals: [{
    name: 'Board',
    entity: { name: 'Cell', persistence: 'runtime', fields: [{ name: 'id', type: 'string' }] },
    traits: [{
      name: 'Scoped', linkedEntity: 'Cell',
      stateMachine: { states: [{ name: 'idle', isInitial: true }], transitions: [] },
      ticks: [{ name: 'beat', interval: 10, effects: [['emit', 'PULSE', {}], ['render-ui', 'main', { type: 'typography', content: ['i18n/t', 'k'] }]] }],
      emits: [{ event: 'PULSE' }],
    }],
  }],
});

const tick = { event: '$TICK_beat', runTick: { trait: 'Scoped', tick: 'beat' }, targetTrait: 'Scoped', entityId: 'c1' };

test('a tick fires only while its trait is in the tick mount, and the mount is set by JSON', async () => {
  const server = new Server(tickSchema);
  const rows = [];
  const paused = await drive(server, 'Board', tick, rows);
  assert.deepEqual(paused.emittedEvents, []);
  server.setTickMount('["Scoped"]');
  const fired = await drive(server, 'Board', tick, rows);
  assert.deepEqual(fired.emittedEvents.map((e) => e.event), ['PULSE']);
  server.setTickMount('[]');
  assert.deepEqual((await drive(server, 'Board', tick, rows)).emittedEvents, []);
  assert.throws(() => server.setTickMount('not json'), /traits_json/);
});

test('the program messages are set by JSON and picked by the request locale', async () => {
  const server = new Server(tickSchema);
  server.setTickMount('["Scoped"]');
  server.setMessages('["en","ar"]', JSON.stringify({ en: { k: 'Hello' }, ar: { k: 'مرحبا' } }));
  const ar = await drive(server, 'Board', { ...tick, locale: 'ar' }, []);
  assert.match(JSON.stringify(ar.clientEffects), /مرحبا/);
  const en = await drive(server, 'Board', { ...tick, locale: 'en' }, []);
  assert.match(JSON.stringify(en.clientEffects), /Hello/);
  assert.throws(() => server.setMessages('{', '{}'), /locales_json/);
});
