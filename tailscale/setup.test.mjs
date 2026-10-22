import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { settings } from './setup.mjs';

// Our organizations' entries: the tag is the tailnet policy's, the client
// ID the calling repository's variable, and no proxy is written here.
test('known organizations supply their tags, and the client ID comes from the caller', () => {
  for (const owner of ['jmarrero-forge', 'jmarrero']) {
    const result = settings(owner, { 'oauth-client-id': 'custom' });
    assert.deepEqual(result, {
      'oauth-client-id': 'custom', audience: 'api.tailscale.com/custom', tags: 'tag:agentic-job', proxy: '',
    });
  }
});

test('each setting can be overridden and audience follows an overridden client', () => {
  assert.deepEqual(settings('jmarrero-forge', {
    'oauth-client-id': 'custom', tags: 'tag:custom', proxy: 'http://example.test',
  }), {
    'oauth-client-id': 'custom', audience: 'api.tailscale.com/custom',
    tags: 'tag:custom', proxy: 'http://example.test',
  });
  assert.equal(settings('jmarrero-forge', { 'oauth-client-id': 'custom', audience: 'custom' }).audience, 'custom');
  assert.equal(settings('jmarrero-forge', { 'oauth-client-id': 'custom', tags: '' }).tags, 'tag:agentic-job');
});

test('an entry without a client ID fails clearly unless the caller gives one', () => {
  for (const owner of ['jmarrero-forge', 'jmarrero']) {
    for (const overrides of [{}, { tags: 'tag:custom' }, { 'oauth-client-id': '' }]) {
      assert.throws(() => settings(owner, overrides), new RegExp(`No oauth-client-id for GitHub organization ${owner}:`));
    }
  }
});

test('unknown owners and output injection fail clearly', () => {
  for (const owner of ['unknown', '__proto__', 'constructor']) {
    assert.throws(() => settings(owner), /No tailnet settings for GitHub organization/);
  }
  assert.throws(() => settings('jmarrero-forge', { 'oauth-client-id': 'custom', proxy: 'url\ninjected=value' }), /single-line/);
});

test('an unlisted organization joins with its own client ID and tags, both', () => {
  assert.deepEqual(settings('other-org', { 'oauth-client-id': 'other', tags: 'tag:other' }), {
    'oauth-client-id': 'other', audience: 'api.tailscale.com/other', tags: 'tag:other', proxy: '',
  });
  assert.equal(settings('other-org', {
    'oauth-client-id': 'other', tags: 'tag:other', proxy: 'http://example.test',
  }).proxy, 'http://example.test');
  for (const overrides of [{ 'oauth-client-id': 'other' }, { tags: 'tag:other' },
    { 'oauth-client-id': 'other', tags: '' }, { audience: 'a', proxy: 'http://example.test' }]) {
    assert.throws(() => settings('other-org', overrides), /No tailnet settings/);
  }
  for (const owner of ['__proto__', 'constructor']) {
    assert.equal(settings(owner, { 'oauth-client-id': 'other', tags: 'tag:other' }).tags, 'tag:other');
  }
  assert.throws(() => settings('other-org', { 'oauth-client-id': 'other', tags: 'tag:a\ninjected=value' }), /single-line/);
});

// The wrappers pass the calling repository's variables, never a
// dispatcher's input, so that another deployment sets its login once.
test('both wrappers join with the repository variables for the login', () => {
  for (const file of ['jmarrero-agentic-job.yml', 'jmarrero-review.yml']) {
    const text = readFileSync(new URL(`../.github/workflows/${file}`, import.meta.url), 'utf8');
    const join = text.split('- name: Join the tailnet\n')[1].split(/\n {6}- /)[0];
    assert.match(join, /\n {10}oauth-client-id: \$\{\{ vars\.AGENTIC_JOB_TAILSCALE_OAUTH_CLIENT_ID \}\}\n/, file);
    assert.match(join, /\n {10}tags: \$\{\{ vars\.AGENTIC_JOB_TAILSCALE_TAGS \}\}(\n|$)/, file);
    assert.doesNotMatch(join, /inputs\./, file);
  }
});

test('the setup subprocess appends outputs and rejects injection before writing', () => {
  const directory = mkdtempSync(join(homedir(), 'tailscale-output-'));
  const output = join(directory, 'output');
  const prefix = 'existing=value\n';
  try {
    for (const scenario of [
      { owner: 'jmarrero-forge', overrides: { OAUTH_CLIENT_ID: 'custom' }, status: 0 },
      { owner: 'jmarrero-forge', overrides: {}, status: 1, error: /No oauth-client-id/ },
      { owner: 'jmarrero', overrides: {
        OAUTH_CLIENT_ID: 'custom', AUDIENCE: 'custom-audience',
        TAGS: 'tag:custom', PROXY: 'http://example.test',
      }, status: 0 },
      { owner: 'unknown', overrides: {}, status: 1, error: /No tailnet settings/ },
      { owner: 'unknown', overrides: { OAUTH_CLIENT_ID: 'other', TAGS: 'tag:other' }, status: 0 },
      { owner: 'unknown', overrides: { OAUTH_CLIENT_ID: 'other' }, status: 1, error: /No tailnet settings/ },
      ...['\n', '\r', '\r\n'].map(newline => ({
        owner: 'jmarrero-forge', overrides: { OAUTH_CLIENT_ID: 'custom', PROXY: `url${newline}injected=value` },
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
