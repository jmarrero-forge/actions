import assert from 'node:assert/strict';
import { test } from 'node:test';
import { runnerEnv } from './runner-env.mjs';

test('runner.env passes only trust-store paths under the proxy directory', () => {
  const file = 'NODE_EXTRA_CA_CERTS=/etc/egress-proxy/ca.pem\nREQUESTS_CA_BUNDLE=/etc/egress-proxy/ca-bundle.pem\n';
  assert.equal(runnerEnv(`# exported\n${file}\n`), file);
  for (const bad of ['', 'NODE_OPTIONS=--require=/tmp/x.js', 'BASH_ENV=/etc/egress-proxy/ca.pem',
    'NODE_EXTRA_CA_CERTS=/tmp/ca.pem', 'NODE_EXTRA_CA_CERTS=/etc/egress-proxy/..',
    'NODE_EXTRA_CA_CERTS=/etc/egress-proxy/ca.pem\nX<<EOF']) {
    assert.throws(() => runnerEnv(bad), /runner.env/);
  }
});
