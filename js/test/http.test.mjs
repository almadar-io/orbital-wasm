import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createECDH, createHash, createHmac, createPublicKey, verify } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { pkgDir, loadPkg } from './pkg.mjs';

const { httpDriverStart, httpDriverResume, initSync } = await loadPkg();
initSync({ module: readFileSync(join(pkgDir, 'orbital_wasm_bg.wasm')) });

const origin = 'https://api.example.com';
const tokenUrl = 'https://auth.example.com/token';
const resume = (progress, answer) => JSON.parse(httpDriverResume(JSON.stringify(progress.driver), JSON.stringify(answer)));
const secret = (value) => ({ kind: 'secret', result: { ok: value } });
const reply = (body) => ({ kind: 'http', result: { ok: { status: 200, headers: [], body: { kind: 'text', text: body } } } });

test('built wasm encrypts the RFC Web Push vector and independently verifies stamped VAPID', () => {
  const privateKey = Buffer.alloc(32, 9);
  const signer = createECDH('prime256v1');
  signer.setPrivateKey(privateKey);
  const publicKey = signer.getPublicKey();
  const call = { method: 'POST', origin, path: '/search', key: 'push', nowMs: 1700000000000, request: {
    webpush: { subscription: { endpoint: `${origin}/search`, keys: {
      p256dh: 'BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4', auth: 'BTBZMqHH6r4Tts7J_aSIgg',
    } }, payload: 'When I grow up, I want to be a watermelon' },
    auth: { scheme: 'vapid', privateKey: 'PRIVATE', publicKey: publicKey.toString('base64url'), subject: 'mailto:owner@example.com' },
  } };
  const first = JSON.parse(httpDriverStart(JSON.stringify(call), JSON.stringify([origin])));
  assert.deepEqual(first.step.ask, { kind: 'secret', name: 'PRIVATE' });
  const entropyAsk = resume(first, secret(privateKey.toString('base64url')));
  assert.deepEqual(entropyAsk.step.ask, { kind: 'random', bytes: 48 });
  const entropy = Buffer.concat([Buffer.from('yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw', 'base64url'), Buffer.from('DGv6ra1nlYgDCS1FRnbzlw', 'base64url')]);
  const sent = resume(entropyAsk, { kind: 'random', result: { ok: entropy.toString('base64') } });
  const request = sent.step.ask.request;
  const headers = Object.fromEntries(request.headers.map(([name, value]) => [name.toLowerCase(), value]));
  assert.equal(headers.ttl, '2419200');
  assert.equal(headers.urgency, 'normal');
  assert.equal(headers['content-encoding'], 'aes128gcm');
  assert.equal(request.body.kind, 'base64');
  assert.equal(Buffer.from(request.body.data, 'base64').toString('base64url'), 'DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN');
  const jwt = headers.authorization.split('vapid t=')[1].split(', k=')[0];
  const [header, claims, signature] = jwt.split('.');
  assert.deepEqual(JSON.parse(Buffer.from(claims, 'base64url').toString()), { aud: origin, exp: 1700043200, sub: 'mailto:owner@example.com' });
  const key = createPublicKey({ format: 'jwk', key: { kty: 'EC', crv: 'P-256', x: publicKey.subarray(1, 33).toString('base64url'), y: publicKey.subarray(33).toString('base64url') } });
  assert.equal(verify('sha256', Buffer.from(`${header}.${claims}`), { key, dsaEncoding: 'ieee-p1363' }, Buffer.from(signature, 'base64url')), true);
  assert.equal(JSON.stringify(sent.driver).includes(privateKey.toString('base64url')), false);
  assert.equal(resume(sent, reply('ok')).step.result.ok.key, 'push');
});

test('built wasm signs SigV4 with the dispatch stamp and presigns without transport', () => {
  const hmac = (key, value) => createHmac('sha256', key).update(value).digest();
  const hash = (value) => createHash('sha256').update(value).digest('hex');
  const scope = '20150830/us-east-1/s3/aws4_request';
  const signingKey = hmac(hmac(hmac(hmac('AWS4dummy-secret', '20150830'), 'us-east-1'), 's3'), 'aws4_request');
  for (const presign of [false, true]) {
    const call = { method: 'GET', origin, path: '/bucket/a%20b', key: 'job', nowMs: 1440938160000,
      request: { query: { q: 'a b' }, auth: { scheme: 'aws-sigv4', accessKeyId: 'ID', secretAccessKey: 'SECRET', region: 'us-east-1', service: 's3' },
        ...(presign ? { presign: { expiresSec: 900 } } : {}) } };
    const first = JSON.parse(httpDriverStart(JSON.stringify(call), JSON.stringify([origin])));
    assert.deepEqual(first.step.ask, { kind: 'secret', name: 'ID' });
    const second = resume(first, secret('AKIDEXAMPLE'));
    assert.deepEqual(second.step.ask, { kind: 'secret', name: 'SECRET' });
    const signed = resume(second, secret('dummy-secret'));
    assert.equal(signed.driver.nowMs, call.nowMs);
    assert.equal(JSON.stringify(signed.driver).includes('dummy-secret'), false);
    if (presign) {
      assert.equal(signed.step.kind, 'done');
      assert.equal(signed.step.result.ok.key, 'job');
      const url = new URL(signed.step.result.ok.url);
      assert.equal(url.pathname, '/bucket/a%20b');
      assert.equal(url.searchParams.get('X-Amz-Date'), '20150830T123600Z');
      assert.equal(url.searchParams.get('X-Amz-Expires'), '900');
      const signature = url.searchParams.get('X-Amz-Signature');
      url.searchParams.delete('X-Amz-Signature');
      const encoded = [...url.searchParams].map(([key, value]) => [encodeURIComponent(key), encodeURIComponent(value)]).sort();
      const query = encoded.map(([key, value]) => `${key}=${value}`).join('&');
      const canonical = `GET\n/bucket/a%20b\n${query}\nhost:api.example.com\n\nhost\nUNSIGNED-PAYLOAD`;
      assert.equal(signature, hmac(signingKey, `AWS4-HMAC-SHA256\n20150830T123600Z\n${scope}\n${hash(canonical)}`).toString('hex'));
    } else {
      const request = signed.step.ask.request;
      const headers = Object.fromEntries(request.headers);
      assert.equal(headers['x-amz-date'], '20150830T123600Z');
      const canonical = `GET\n/bucket/a%20b\nq=a%20b\nhost:api.example.com\nx-amz-content-sha256:${hash('')}\nx-amz-date:20150830T123600Z\n\nhost;x-amz-content-sha256;x-amz-date\n${hash('')}`;
      const signature = hmac(signingKey, `AWS4-HMAC-SHA256\n20150830T123600Z\n${scope}\n${hash(canonical)}`).toString('hex');
      assert.equal(headers.authorization, `AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/${scope}, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature=${signature}`);
    }
  }
});

for (const [alg, fixture] of [['RS256', 'rs256-pkcs8.pem'], ['ES256', 'es256-pkcs8.pem']]) {
  test(`built wasm signs ${alg} JWTs and preserves credential/cache isolation`, () => {
    const pem = readFileSync(new URL(`../../tests/fixtures/${fixture}`, import.meta.url), 'utf8');
    const claims = { scope: 'calendar', aud: tokenUrl, exp: 4600, iat: 1000 };
    const call = { method: 'GET', origin, path: '/search', key: 'job', request: { auth: {
      scheme: 'jwt-bearer', key: { credential: 'ACCOUNT', scope: 'user' }, alg, claims, tokenUrl,
      expiresMarginMs: 300000, storeAs: { name: 'google:token', scope: 'user' },
    } } };
    const first = JSON.parse(httpDriverStart(JSON.stringify(call), JSON.stringify([origin, 'https://auth.example.com'])));
    assert.deepEqual(first.step.ask, { kind: 'secret', name: 'google:token', scope: 'user' });
    const key = resume(first, secret(null));
    assert.deepEqual(key.step.ask, { kind: 'secret', name: 'ACCOUNT', scope: 'user' });
    const grant = resume(key, secret(JSON.stringify({ client_email: 'service@example.com', private_key: pem })));
    assert.equal(grant.step.ask.request.url, tokenUrl);
    const form = new URLSearchParams(grant.step.ask.request.body.text);
    assert.equal(form.get('grant_type'), 'urn:ietf:params:oauth:grant-type:jwt-bearer');
    const assertion = form.get('assertion');
    assert.equal(typeof assertion, 'string');
    const [header, payload, signature] = assertion.split('.');
    assert.deepEqual(JSON.parse(Buffer.from(header, 'base64url').toString()), { alg });
    assert.deepEqual(JSON.parse(Buffer.from(payload, 'base64url').toString()), { iss: 'service@example.com', ...claims });
    assert.equal(verify('sha256', Buffer.from(`${header}.${payload}`), {
      key: createPublicKey(pem), dsaEncoding: 'ieee-p1363',
    }, Buffer.from(signature, 'base64url')), true);
    const stored = resume(grant, reply(JSON.stringify({ access_token: 'T1', expires_in: 3600 })));
    assert.deepEqual(stored.step.ask, { kind: 'secretSet', name: 'google:token', value: 'T1', ttlMs: 3300000, scope: 'user' });
    for (const progress of [grant, stored]) {
      const serialized = JSON.stringify(progress.driver);
      assert.equal(serialized.includes('service@example.com'), false);
      assert.equal(serialized.includes('PRIVATE KEY'), false);
      assert.equal(serialized.includes('T1'), false);
    }
    const sent = resume(stored, { kind: 'secretSet', result: { ok: null } });
    assert.deepEqual(sent.step.ask.request.headers.find(([name]) => name === 'Authorization'), ['Authorization', 'Bearer T1']);
    assert.equal(resume(sent, reply('ok')).step.result.ok.key, 'job');
    const missing = resume(key, secret(null));
    assert.equal(missing.step.result.error.kind, 'unconfigured');
    assert.equal(missing.step.result.error.credential, 'ACCOUNT');
    assert.equal(missing.step.result.error.key, 'job');
  });
}

test('built wasm keeps same-named app and user slots distinct', () => {
  const call = { method: 'GET', origin, path: '/', request: { auth: { scheme: 'basic',
    user: { credential: 'SAME', scope: 'app' }, credential: { credential: 'SAME', scope: 'user' },
  } } };
  const first = JSON.parse(httpDriverStart(JSON.stringify(call), JSON.stringify([origin])));
  assert.deepEqual(first.step.ask, { kind: 'secret', name: 'SAME' });
  const second = resume(first, secret('APP'));
  assert.deepEqual(second.step.ask, { kind: 'secret', name: 'SAME', scope: 'user' });
  const sent = resume(second, secret('USER'));
  assert.deepEqual(sent.step.ask.request.headers.find(([name]) => name === 'Authorization'), ['Authorization', 'Basic QVBQOlVTRVI=']);
});

test('token-exchange cache identity retains credential scope and permission boundaries', () => {
  const asks = [];
  for (const scope of ['app', 'user']) {
    for (const permission of ['calendar', 'drive']) {
      const call = { method: 'GET', origin, path: '/', request: { auth: { scheme: 'token-exchange', tokenUrl, grant: 'refresh_token',
        body: { form: { refresh_token: { credential: 'REFRESH', scope }, scope: permission } },
      } } };
      const first = JSON.parse(httpDriverStart(JSON.stringify(call), JSON.stringify([origin, 'https://auth.example.com'])));
      assert.equal(first.step.ask.scope ?? 'app', scope);
      asks.push(first.step.ask.name);
    }
  }
  assert.equal(new Set(asks).size, 4);
});

test('built wasm refuses malformed headers before a host ask and echoes the key', () => {
  const call = { method: 'GET', origin, path: '/', key: 'job', request: { headers: { 'X-Test': 'line\r\ninjection' } } };
  const refused = JSON.parse(httpDriverStart(JSON.stringify(call), JSON.stringify([origin])));
  assert.equal(refused.driver, null);
  assert.equal(refused.step.result.error.kind, 'refused');
  assert.equal(refused.step.result.error.key, 'job');
});

test('built wasm refuses credential-derived header injection without leaking its value', () => {
  const call = { method: 'GET', origin, path: '/', key: 'job', request: { auth: { scheme: 'bearer', credential: 'KEY' } } };
  const first = JSON.parse(httpDriverStart(JSON.stringify(call), JSON.stringify([origin])));
  assert.deepEqual(first.step.ask, { kind: 'secret', name: 'KEY' });
  const value = 'token\r\nInjected: value';
  const refused = resume(first, secret(value));
  assert.equal(refused.step.kind, 'done');
  assert.equal(refused.step.result.error.kind, 'refused');
  assert.equal(refused.step.result.error.key, 'job');
  assert.equal(refused.step.result.error.error.includes(value), false);
});
