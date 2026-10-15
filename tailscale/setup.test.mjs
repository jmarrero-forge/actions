import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { settings } from './setup.mjs';

test('known organizations supply join settings and proxy', () => {
  for (const owner of ['bootc-dev', 'cgwalters-forge-stage', 'cgwalters-forge']) {
    const result = settings(owner);
    assert.equal(result.audience, `api.tailscale.com/${result['oauth-client-id']}`);
    assert.equal(result.tags, 'tag:bootc-dev-sandbox');
    assert.equal(result.proxy, 'http://100.121.0.115:18080');
  }
});

test('each setting can be overridden and audience follows an overridden client', () => {
  assert.deepEqual(settings('bootc-dev', {
    'oauth-client-id': 'custom', tags: 'tag:custom', proxy: 'http://example.test',
  }), {
    'oauth-client-id': 'custom', audience: 'api.tailscale.com/custom',
    tags: 'tag:custom', proxy: 'http://example.test',
  });
  assert.equal(settings('bootc-dev', { audience: 'custom' }).audience, 'custom');
  assert.equal(settings('bootc-dev', { tags: '' }).tags, 'tag:bootc-dev-sandbox');
});

test('an entry with empty settings takes them from the inputs, and fails clearly without', () => {
  for (const overrides of [{}, { 'oauth-client-id': 'custom' }, { tags: 'tag:custom' }]) {
    assert.throws(() => settings('jmarrero-forge', overrides), /No (oauth-client-id|tags) for GitHub organization jmarrero-forge/);
  }
  assert.deepEqual(settings('jmarrero-forge', { 'oauth-client-id': 'custom', tags: 'tag:custom' }), {
    'oauth-client-id': 'custom', audience: 'api.tailscale.com/custom', tags: 'tag:custom', proxy: '',
  });
});

test('unknown owners and output injection fail clearly', () => {
  for (const owner of ['unknown', '__proto__', 'constructor']) {
    assert.throws(() => settings(owner), /No tailnet settings for GitHub organization/);
  }
  assert.throws(() => settings('bootc-dev', { proxy: 'url\ninjected=value' }), /single-line/);
});

test('the setup subprocess appends outputs and rejects injection before writing', () => {
  const directory = mkdtempSync(join(homedir(), 'tailscale-output-'));
  const output = join(directory, 'output');
  const prefix = 'existing=value\n';
  try {
    for (const scenario of [
      { owner: 'bootc-dev', overrides: {}, status: 0 },
      { owner: 'cgwalters-forge', overrides: {
        OAUTH_CLIENT_ID: 'custom', AUDIENCE: 'custom-audience',
        TAGS: 'tag:custom', PROXY: 'http://example.test',
      }, status: 0 },
      { owner: 'unknown', overrides: {}, status: 1, error: /No tailnet settings/ },
      ...['\n', '\r', '\r\n'].map(newline => ({
        owner: 'bootc-dev', overrides: { PROXY: `url${newline}injected=value` },
        status: 1, error: /single-line/,
      })),
    ]) {
      writeFileSync(output, prefix);
      const result = spawnSync(process.execPath, [fileURLToPath(new URL('./setup.mjs', import.meta.url))], {
        encoding: 'utf8',
        env: {
          ...process.env, OAUTH_CLIENT_ID: '', AUDIENCE: '', TAGS: '', PROXY: '',
          REPOSITORY_OWNER: scenario.owner, GITHUB_OUTPUT: output, ...scenario.overrides,
        },
      });
      assert.ifError(result.error);
      assert.equal(result.status, scenario.status, result.stderr);
      if (scenario.error) {
        assert.match(result.stderr, scenario.error);
        assert.equal(readFileSync(output, 'utf8'), prefix);
      } else {
        const overrides = Object.fromEntries(Object.entries(scenario.overrides)
          .map(([key, value]) => [key.toLowerCase().replaceAll('_', '-'), value]));
        const expected = Object.entries(settings(scenario.owner, overrides))
          .map(([key, value]) => `${key}=${value}\n`).join('');
        assert.equal(readFileSync(output, 'utf8'), prefix + expected);
      }
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
