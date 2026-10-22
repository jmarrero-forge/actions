import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// ALLOWED, if given, is a path of example.com that an `egress-policy`
// rule allows POSTs to: that POST must reach the origin, while one to the
// root is still refused.
export async function probe(request = fetch, env = process.env, allowed = '') {
  // Node fetch must use transparent interception and the exported per-job CA,
  // not explicit proxy settings. Each request has its own finite deadline.
  assert.ok(!env.NODE_USE_ENV_PROXY, 'NODE_USE_ENV_PROXY must be unset');
  const send = async (method, path, origin = 'https://example.com') => {
    const response = await request(`${origin}${path}`, {
      method, redirect: 'error', signal: AbortSignal.timeout(15000),
      ...(method === 'POST' ? { body: 'probe' } : {}),
    });
    await response.body?.cancel();
    return response;
  };
  for (const method of ['GET', 'POST']) {
    const response = await send(method, '/');
    assert.equal(response.status, method === 'GET' ? 200 : 403);
    if (method === 'POST') {
      assert.match(response.headers.get('x-egress-denied') || '', /not in the write allowlist/);
    }
    console.log(`${method}: ${method === 'GET' ? 'allowed' : 'refused by proxy'}`);
  }
  if (allowed) {
    // Whatever the origin answers, the proxy let it through; a 502 is the
    // proxy's own answer when it could not forward the request.
    const response = await send('POST', allowed);
    assert.equal(response.headers.get('x-egress-denied'), null,
      `POST ${allowed} refused by proxy: ${response.headers.get('x-egress-denied')}`);
    assert.notEqual(response.status, 502, `POST ${allowed} did not reach the origin`);
    console.log(`POST ${allowed}: allowed by rule (origin answered ${response.status})`);
    // The pinned image allows npm audit writes by default. The fixture
    // omits them: merging instead of replacing would leave this write channel
    // open. Require the proxy's refusal, not an origin error or a timeout.
  }
  // Positive control in the default-policy job: if the image stops allowing
  // this endpoint, it no longer distinguishes replacement from merging.
  const defaultOnly = await send('POST', '/-/npm/v1/security/advisories/bulk',
    'https://registry.npmjs.org');
  if (allowed) {
    assert.equal(defaultOnly.status, 403);
    assert.match(defaultOnly.headers.get('x-egress-denied') || '', /not in the write allowlist/);
    console.log('npm audit POST: refused by replacement policy');
  } else {
    assert.equal(defaultOnly.headers.get('x-egress-denied'), null);
    assert.notEqual(defaultOnly.status, 502, 'default npm audit POST did not reach the origin');
    console.log('npm audit POST: allowed by default policy');
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  await probe(fetch, process.env, process.argv[2]);
}
