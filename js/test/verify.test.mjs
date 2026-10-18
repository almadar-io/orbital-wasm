import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { pkgDir, loadPkg } from './pkg.mjs';

const { initSync, verifyPlan, verifyExpect, verifyJudge, verifyCredit } = await loadPkg();
initSync({ module: readFileSync(join(pkgDir, 'orbital_wasm_bg.wasm')) });

const here = dirname(fileURLToPath(import.meta.url));
const readJson = (path) => JSON.parse(readFileSync(path, 'utf8'));

const verifyFixtures = join(here, '../../tests/fixtures/verify');

for (const name of ['entity_payload_skeleton', '02-state-machine', '04-complex-guards', 'json-array', 'precondition']) {
  test(`verifyPlan returns the native planner's plan for ${name}`, () => {
    const program = readFileSync(join(verifyFixtures, `${name}.json`), 'utf8');
    const golden = readJson(join(verifyFixtures, `${name}.plan.json`));
    assert.equal(golden.outcome, 'planned');
    assert.ok(golden.steps.length > 0);
    const { contractEvents, ...planned } = JSON.parse(verifyPlan(program, ''));
    assert.deepEqual(planned, golden);
    assert.deepEqual(contractEvents, { available: false, steps: [] }, 'no language data is installed in this host');
  });
}

test('verifyExpect and verifyJudge decide a planned step from its oracle', () => {
  const program = readFileSync(join(verifyFixtures, '02-state-machine.json'), 'utf8');
  const plan = JSON.parse(verifyPlan(program, ''));
  const step = plan.steps.find((s) => s.reply === undefined && s.arm === undefined && !s.guarded && s.payloadCase === 'success' && s.acceptStates === undefined);
  const expectation = verifyExpect(JSON.stringify(step));
  const judge = (stateAfter) =>
    JSON.parse(verifyJudge(expectation, JSON.stringify({ stateBefore: step.from, stateAfter, effects: [] })));
  assert.deepEqual(judge(step.to), { status: 'pass', reason: { kind: 'reached' } });
  assert.deepEqual(judge('__nowhere__'), { status: 'fail', reason: { kind: 'wrong-state', actual: '__nowhere__' } });
  assert.throws(() => verifyJudge('{}', '{}'));
});

test('control: a broken program or trait scope is a typed refusal, not a throw', () => {
  const program = readFileSync(join(verifyFixtures, '02-state-machine.json'), 'utf8');
  const broken = JSON.parse(verifyPlan('{not json', ''));
  assert.equal(broken.outcome, 'refused');
  assert.equal(broken.failure.kind, 'invalid-program');
  const scoped = JSON.parse(verifyPlan(program, JSON.stringify({ traits: ['NoSuchTrait'] })));
  assert.equal(scoped.failure.kind, 'unknown-trait');
  assert.equal(scoped.failure.name, 'NoSuchTrait');
  assert.match(scoped.failure.available, /OrderLifecycle/);
  assert.equal(JSON.parse(verifyPlan(program, '[1]')).failure.kind, 'invalid-input');
});

test('verifyCredit credits a failure route from another step\'s dispatch, and throws on bad input', () => {
  const hop = { trait: 'Caller', from: 'waiting', event: 'CALL_FAILED', to: 'failed' };
  const steps = [
    { trait: 'Caller', from: 'idle', event: 'CALL', fired: [hop] },
    { trait: 'Caller', from: 'waiting', event: 'CALL_FAILED', reply: 'failure', fired: [] },
  ];
  assert.deepEqual(JSON.parse(verifyCredit(JSON.stringify(steps))), [{ index: 1, stateBefore: 'waiting', stateAfter: 'failed' }]);
  assert.deepEqual(JSON.parse(verifyCredit(JSON.stringify([steps[1]]))), []);
  assert.throws(() => verifyCredit('{not json'));
});

test('verifyPlan carries the declared-value config sweep and the boot credits', () => {
  const program = readFileSync(join(verifyFixtures, 'config.json'), 'utf8');
  const plan = JSON.parse(verifyPlan(program, ''));
  const gauge = plan.configVariants[0];
  assert.deepEqual(gauge.variants.map((v) => v.label), ['animated = false', 'format = bar', 'format = numeric']);
  assert.deepEqual(gauge.noDeclaredValues, ['className', 'current']);
  assert.deepEqual(plan.initCredits, [{ trait: 'Gauge', state: 'idle', coverageKey: 'Gauge:idle+INIT->idle' }]);
});
