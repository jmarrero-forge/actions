import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { admitted } from './admit.mjs';

const owners = JSON.parse(readFileSync(new URL('./owners.json', import.meta.url), 'utf8'));

test('owners.json maps logins to numeric ids', () => {
  assert.ok(Object.keys(owners).length > 0);
  for (const [login, id] of Object.entries(owners)) assert.ok(Number.isSafeInteger(id) && id > 0, login);
});

test('only a listed owner id is admitted', () => {
  for (const id of Object.values(owners)) assert.equal(admitted(String(id)), true);
  const listed = String(Object.values(owners)[0]);
  // A login is no id, and only the exact decimal string matches.
  for (const value of [undefined, '', '0', '1', Object.keys(owners)[0], ` ${listed}`, `${listed} `, `0${listed}`, `${listed}.0`, `[${listed}]`]) {
    assert.equal(admitted(value), false, JSON.stringify(value));
  }
});

test('the script fails for an owner that is not listed', () => {
  const script = fileURLToPath(new URL('./admit.mjs', import.meta.url));
  const run = id => spawnSync(process.execPath, [script], { encoding: 'utf8', env: { ...process.env, OWNER: 'caller', OWNER_ID: id } });
  assert.equal(run(String(Object.values(owners)[0])).status, 0);
  const refused = run('1');
  assert.equal(refused.status, 1);
  assert.match(refused.stdout, /::error::caller \(id 1\) is not an owner admit\/owners\.json lists/);
});
