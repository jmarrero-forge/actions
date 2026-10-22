import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { settings } from './setup.mjs';

// cgwalters-forge's broker moved; the others stay on the old host until
// their tailnet membership of the new one is confirmed.
test('known organizations supply join settings and proxy', () => {
  for (const [owner, proxy] of [
    ['bootc-dev', 'http://100.114.231.4:18080'],
    ['cgwalters-forge-stage', 'http://100.114.231.4:18080'],
    ['cgwalters-forge', 'http://100.114.231.4:18080'],
  ]) {
    const result = settings(owner);
    assert.equal(result.audience, `api.tailscale.com/${result['oauth-client-id']}`);
    assert.equal(result.tags, 'tag:bootc-dev-sandbox');
    assert.equal(result.proxy, proxy);
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

test('unknown owners and output injection fail clearly', () => {
  for (const owner of ['unknown', '__proto__', 'constructor']) {
    assert.throws(() => settings(owner), /No tailnet settings for GitHub organization/);
  }
  assert.throws(() => settings('bootc-dev', { proxy: 'url\ninjected=value' }), /single-line/);
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
  for (const file of ['cgwalters-agentic-job.yml', 'cgwalters-review.yml']) {
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
      { owner: 'bootc-dev', overrides: {}, status: 0 },
      { owner: 'cgwalters-forge', overrides: {
        OAUTH_CLIENT_ID: 'custom', AUDIENCE: 'custom-audience',
        TAGS: 'tag:custom', PROXY: 'http://example.test',
      }, status: 0 },
      { owner: 'unknown', overrides: {}, status: 1, error: /No tailnet settings/ },
      { owner: 'unknown', overrides: { OAUTH_CLIENT_ID: 'other', TAGS: 'tag:other' }, status: 0 },
      { owner: 'unknown', overrides: { OAUTH_CLIENT_ID: 'other' }, status: 1, error: /No tailnet settings/ },
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
