import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { generateKeyPairSync, createVerify } from 'node:crypto';
import { existsSync, mkdtempSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { test } from 'node:test';
import { MARGIN_MS, appJwt, mint, permissions, repositories, revoke, statePath, token } from './remint.mjs';

// A key made for the test run, never one written down.
const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
const now = Date.parse('2026-10-19T12:00:00Z');
const hour = 60 * 60 * 1000;

function env(extra = {}) {
  return {
    APP_ID: '12345', PRIVATE_KEY: privateKey.export({ type: 'pkcs1', format: 'pem' }),
    OWNER: 'cgwalters-forge', REPOSITORIES: 'actions,\ntracker', PERMISSION_ISSUES: 'write',
    GITHUB_API_URL: 'https://api.example', RUNNER_TEMP: mkdtempSync(join(tmpdir(), 'app-token-')),
    ...extra,
  };
}

// The two calls of a mint, answering with tokens that expire at `expiries`.
function github(expiries) {
  const calls = [];
  const request = async (url, options) => {
    calls.push({ url, options });
    if (url.endsWith('/installation')) return Response.json({ id: 77 });
    const n = calls.filter(call => call.url.endsWith('/access_tokens')).length;
    return Response.json({ token: `token-${n}`, expires_at: new Date(expiries[n - 1]).toISOString() });
  };
  return { calls, request };
}

test('the JWT is the app\'s, signed, and lasts under the ten minutes GitHub allows', () => {
  const [header, body, signature] = appJwt('12345', privateKey, now).split('.');
  assert.deepEqual(JSON.parse(Buffer.from(header, 'base64url')), { alg: 'RS256', typ: 'JWT' });
  const claims = JSON.parse(Buffer.from(body, 'base64url'));
  assert.deepEqual(claims, { iat: now / 1000 - 60, exp: now / 1000 + 480, iss: '12345' });
  assert.ok(createVerify('RSA-SHA256').update(`${header}.${body}`).verify(publicKey, signature, 'base64url'));
});

test('a mint asks the owner\'s installation for the named repositories only', async () => {
  const { calls, request } = github([now + hour]);
  assert.deepEqual(await mint(env(), request, now), { token: 'token-1', expires_at: new Date(now + hour).toISOString() });
  assert.equal(calls[0].url, 'https://api.example/repos/cgwalters-forge/actions/installation');
  assert.match(calls[0].options.headers.Authorization, /^Bearer [\w-]+\.[\w-]+\.[\w-]+$/);
  assert.equal(calls[1].url, 'https://api.example/app/installations/77/access_tokens');
  assert.equal(calls[1].options.method, 'POST');
  assert.deepEqual(JSON.parse(calls[1].options.body),
    { repositories: ['actions', 'tracker'], permissions: { issues: 'write' } });
});

test('a mint refuses a missing input, an empty scope and a failed or odd response', async () => {
  for (const missing of ['APP_ID', 'PRIVATE_KEY', 'OWNER', 'REPOSITORIES', 'PERMISSION_ISSUES']) {
    await assert.rejects(mint(env({ [missing]: '' }), github([]).request, now), /are required/);
  }
  await assert.rejects(mint(env({ REPOSITORIES: ' ,\n' }), github([]).request, now), /are required/);
  await assert.rejects(mint(env(), async () => new Response(null, { status: 404 }), now), /Installation lookup failed: HTTP 404/);
  const answers = [Response.json({ id: 77 }), new Response(null, { status: 422 })];
  await assert.rejects(mint(env(), async () => answers.shift(), now), /Token request failed: HTTP 422/);
  const odd = [Response.json({ id: 77 }), Response.json({ token: 't' })];
  await assert.rejects(mint(env(), async () => odd.shift(), now), /token and its expiry/);
});

test('the kept token is reused until it is near expiry, then minted again', async () => {
  const e = env();
  const { calls, request } = github([now + hour, now + 2 * hour]);
  assert.equal(await token(e, {}, request, now), 'token-1');
  assert.equal(statSync(statePath(e)).mode & 0o777, 0o600);
  assert.equal(await token(e, {}, request, now + hour - MARGIN_MS - 1), 'token-1');
  assert.equal(calls.length, 2);
  assert.equal(await token(e, {}, request, now + hour - MARGIN_MS), 'token-2');
  assert.equal(calls.length, 4);
  assert.equal(JSON.parse(readFileSync(statePath(e), 'utf8')).token, 'token-2');
});

test('after a 401 the kept token is dropped, however long it has left', async () => {
  const e = env();
  const { request } = github([now + hour, now + hour]);
  assert.equal(await token(e, {}, request, now), 'token-1');
  assert.equal(await token(e, { unauthorized: true }, request, now), 'token-2');
  assert.equal(await token(e, {}, request, now), 'token-2');
});

test('an unreadable kept token is minted again; with no temporary directory nothing is kept', async () => {
  const e = env();
  writeFileSync(statePath(e), 'not json');
  assert.equal(await token(e, {}, github([now + hour]).request, now), 'token-1');
  const bare = env({ RUNNER_TEMP: '' });
  assert.equal(statePath(bare), null);
  const { calls, request } = github([now + hour, now + hour]);
  await token(bare, {}, request, now);
  await token(bare, {}, request, now);
  assert.equal(calls.length, 4);
});

test('calls at once each get a token and leave one whole file, and no temporary one', async () => {
  const e = env();
  const { request } = github([now + hour, now + hour, now + hour]);
  const tokens = await Promise.all([1, 2, 3].map(() => token(e, {}, request, now)));
  assert.deepEqual(tokens.toSorted(), ['token-1', 'token-2', 'token-3']);
  assert.ok(tokens.includes(JSON.parse(readFileSync(statePath(e), 'utf8')).token));
  assert.deepEqual(readdirSync(e.RUNNER_TEMP), [basename(statePath(e))]);
});

test('a one-line key with literal \\n mints as the action would', async () => {
  const oneLine = env().PRIVATE_KEY.trim().replaceAll('\n', '\\n');
  const { calls, request } = github([now + hour]);
  await mint(env({ PRIVATE_KEY: oneLine }), request, now);
  const [header, body, signature] = calls[0].options.headers.Authorization.slice('Bearer '.length).split('.');
  assert.ok(createVerify('RSA-SHA256').update(`${header}.${body}`).verify(publicKey, signature, 'base64url'));
});

test('revoking removes the kept token, then revokes it with itself', async () => {
  const e = env();
  await token(e, {}, github([now + hour]).request, now);
  const calls = [];
  const answer = status => async (url, options) => {
    calls.push({ url, options, kept: existsSync(statePath(e)) });
    return new Response(null, { status });
  };
  await revoke(e, answer(204));
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, 'https://api.example/installation/token');
  assert.equal(calls[0].options.method, 'DELETE');
  assert.equal(calls[0].options.headers.Authorization, 'Bearer token-1');
  assert.equal(calls[0].kept, false);
  await revoke(e, answer(204));
  assert.equal(calls.length, 1, 'nothing kept, nothing to revoke');
  // A token already expired or revoked is fine; any other failure is reported.
  await token(e, {}, github([now + hour]).request, now);
  await revoke(e, answer(401));
  await token(e, {}, github([now + hour]).request, now);
  await assert.rejects(revoke(e, answer(500)), /Token revocation failed: HTTP 500/);
  assert.equal(existsSync(statePath(e)), false);
  await revoke(env({ RUNNER_TEMP: '' }), answer(204));
  assert.equal(calls.length, 3);
});

test('a failed revocation is not reused, and the next --revoke tries it again', async () => {
  const e = env();
  const { request } = github([now + hour, now + hour]);
  assert.equal(await token(e, {}, request, now), 'token-1');
  const timeout = async () => { throw new DOMException('timed out', 'TimeoutError'); };
  await assert.rejects(revoke(e, timeout), /timed out/);
  await assert.rejects(revoke(e, async () => new Response(null, { status: 502 })), /HTTP 502/);
  // Meanwhile the step mints afresh rather than reusing the token being revoked.
  assert.equal(await token(e, {}, request, now), 'token-2');
  const revoked = [];
  await revoke(e, async (url, options) => {
    revoked.push(options.headers.Authorization);
    return new Response(null, { status: 204 });
  });
  assert.deepEqual(revoked, ['Bearer token-1', 'Bearer token-2']);
  assert.deepEqual(readdirSync(e.RUNNER_TEMP), []);
});

test('the command prints the token alone on stdout and its mask on stderr', () => {
  const e = env({ APP_TOKEN_STATE: join(mkdtempSync(join(tmpdir(), 'app-token-')), 'state.json') });
  writeFileSync(e.APP_TOKEN_STATE, JSON.stringify({ token: 'kept', expires_at: new Date(Date.now() + hour).toISOString() }));
  const script = new URL('./remint.mjs', import.meta.url).pathname;
  const run = spawnSync(process.execPath, [script], { env: e, encoding: 'utf8' });
  assert.equal(run.status, 0, run.stderr);
  assert.equal(run.stdout, 'kept\n');
  assert.equal(run.stderr, '::add-mask::kept\n');
  for (const args of [['--force'], ['--revoke', '--unauthorized']]) {
    const bad = spawnSync(process.execPath, [script, ...args], { env: e, encoding: 'utf8' });
    assert.equal(bad.status, 2);
    assert.equal(bad.stdout, '');
  }
});

test('each app and scope keeps its own token', () => {
  const a = env({ RUNNER_TEMP: '/tmp/x' });
  assert.notEqual(statePath(a), statePath({ ...a, REPOSITORIES: 'actions' }));
  assert.notEqual(statePath(a), statePath({ ...a, APP_ID: '1' }));
  assert.notEqual(statePath(a), statePath({ ...a, PERMISSION_CONTENTS: 'write' }));
  assert.deepEqual(permissions({ PERMISSION_ISSUES: 'write', PERMISSION_PULL_REQUESTS: 'read', PERMISSION_ADMINISTRATION: 'write' }),
    { issues: 'write', pull_requests: 'read' });
  assert.equal(statePath({ ...a, APP_TOKEN_STATE: '/s.json' }), '/s.json');
  assert.deepEqual(repositories(' a, b\nc '), ['a', 'b', 'c']);
});
