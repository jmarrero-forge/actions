import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { pinned } from './pin.mjs';

const digest = `sha256:${'0123456789abcdef'.repeat(4)}`;
const signer = {
  repository: 'example/builder',
  workflow: '.github/workflows/release.yml',
  ref: 'refs/heads/main',
};

test('a pin names the image by digest and the signer by workflow identity', () => {
  assert.deepEqual(pinned(`FROM ghcr.io/example/secure-host:v1.2.3@${digest}\n`, signer), {
    image: `ghcr.io/example/secure-host@${digest}`,
    repository: 'example/builder',
    'cert-identity': 'https://github.com/example/builder/.github/workflows/release.yml@refs/heads/main',
    'source-ref': 'refs/heads/main',
  });
  // Without a tag, and with a registry port.
  assert.equal(pinned(`FROM localhost:5000/a/b@${digest}`, signer).image, `localhost:5000/a/b@${digest}`);
});

test('a pin without a digest, or with more than one line, is refused', () => {
  for (const text of [
    'FROM ghcr.io/example/secure-host:v1.2.3',
    `FROM ghcr.io/example/secure-host@sha256:${'0'.repeat(63)}`,
    `FROM ghcr.io/example/secure-host@${digest}\nFROM ghcr.io/other/image@${digest}`,
    `FROM ghcr.io/example/secure-host@${digest} --privileged`,
    `FROM --platform=linux/amd64 ghcr.io/example/secure-host@${digest}`,
    `from ghcr.io/example/secure-host@${digest}`,
    '',
  ]) {
    assert.throws(() => pinned(text, signer), /Containerfile.pin/, JSON.stringify(text));
  }
});

test('a missing or malformed signer is refused, newlines included', () => {
  const text = `FROM ghcr.io/example/secure-host@${digest}`;
  for (const change of [
    { repository: undefined },
    { repository: 'example' },
    { repository: 'example/builder\ninjected=value' },
    { workflow: 'release.yml' },
    { workflow: '.github/workflows/../release.yml\n' },
    { ref: 'main' },
    { ref: 'refs/heads/main\nx=y' },
  ]) {
    assert.throws(() => pinned(text, { ...signer, ...change }), /signer.json/, JSON.stringify(change));
  }
});

test('the checked-in pin and signer are valid and the script writes them as outputs', () => {
  const directory = mkdtempSync(join(tmpdir(), 'secure-host-pin-'));
  try {
    const output = join(directory, 'output');
    const script = fileURLToPath(new URL('./pin.mjs', import.meta.url));
    const result = spawnSync(process.execPath, [script], {
      env: { ...process.env, GITHUB_OUTPUT: output }, encoding: 'utf8',
    });
    assert.equal(result.status, 0, result.stderr);
    const lines = readFileSync(output, 'utf8').trimEnd().split('\n');
    assert.equal(lines.length, 4);
    assert.match(lines[0], /^image=[^\s:]+(?:\/[^\s:@]+)+@sha256:[0-9a-f]{64}$/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
