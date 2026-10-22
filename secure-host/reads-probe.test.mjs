import assert from 'node:assert/strict';
import test from 'node:test';
import { refused } from './reads-probe.mjs';

test('only a refusal or a hidden process proves the rule', () => {
  for (const code of ['EACCES', 'ENOENT']) {
    refused(() => { throw Object.assign(new Error(code), { code }); });
  }
  for (const code of ['ESRCH', 'EIO', 'EPERM']) {
    assert.throws(() => refused(() => { throw Object.assign(new Error(code), { code }); }));
  }
  assert.throws(() => refused(() => Buffer.from('readable')), /still readable/);
});
