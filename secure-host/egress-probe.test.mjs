import assert from 'node:assert/strict';
import { test } from 'node:test';
import { probe } from './egress-probe.mjs';

test('egress probe requires a positive read and a proxy-specific write refusal', async () => {
  for (const outcome of ['proxy', 'origin', 'allowed', 'offline', 'read-denied',
    'default-refused', 'default-unforwarded']) {
    const exercise = probe(async (url, options) => {
      if (url === 'https://registry.npmjs.org/-/npm/v1/security/advisories/bulk') {
        return new Response('', { status: outcome === 'default-unforwarded' ? 502 : 403,
          headers: outcome === 'default-refused' ? { 'x-egress-denied': 'refused' } : {} });
      }
      assert.equal(url, 'https://example.com/');
      assert.equal(options.redirect, 'error');
      assert.ok(options.signal instanceof AbortSignal);
      if (outcome === 'offline') throw new Error('network unavailable');
      if (options.method === 'GET') return new Response('', { status: outcome === 'read-denied' ? 403 : 200 });
      assert.equal(options.body, 'probe');
      return new Response('', { status: outcome === 'allowed' ? 200 : 403,
        headers: ['proxy', 'default-refused', 'default-unforwarded'].includes(outcome)
          ? { 'x-egress-denied': 'POST to an endpoint not in the write allowlist' } : {} });
    }, {});
    if (outcome === 'proxy') await exercise;
    else await assert.rejects(exercise);
  }
  await assert.rejects(probe(() => assert.fail('must not request'), { NODE_USE_ENV_PROXY: '1' }));
});

test('egress probe requires a rule-allowed POST to pass and others still refused', async () => {
  const denied = { 'x-egress-denied': 'POST to an endpoint not in the write allowlist' };
  for (const outcome of ['rule', 'rule-refused', 'all-open', 'unforwarded',
    'merged', 'default-origin-refused', 'default-offline']) {
    const urls = [];
    const exercise = probe(async (url, options) => {
      urls.push(`${options.method} ${url}`);
      if (options.method === 'GET') return new Response('', { status: 200 });
      if (url === 'https://registry.npmjs.org/-/npm/v1/security/advisories/bulk') {
        if (outcome === 'default-offline') throw new Error('network unavailable');
        return new Response('', { status: outcome === 'merged' ? 200 : 403,
          headers: ['merged', 'default-origin-refused'].includes(outcome) ? {} : denied });
      }
      const root = url === 'https://example.com/';
      const refuse = root ? outcome !== 'all-open' : outcome === 'rule-refused';
      // The origin's own refusal of an allowed write still passes.
      const status = refuse || outcome !== 'unforwarded' ? 403 : 502;
      return new Response('', { status, headers: refuse ? denied : {} });
    }, {}, '/allowed');
    if (outcome === 'rule') {
      await exercise;
      assert.deepEqual(urls, ['GET https://example.com/', 'POST https://example.com/',
        'POST https://example.com/allowed',
        'POST https://registry.npmjs.org/-/npm/v1/security/advisories/bulk']);
    } else {
      await assert.rejects(exercise);
    }
  }
});
