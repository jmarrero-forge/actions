import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

const canary = 'SECURE_HOST_READ_PROBE=not-a-secret';

export function refused(read) {
  try {
    read();
  } catch (error) {
    // Setup's hidepid=invisible decides with the same check, so a refused
    // process is missing from /proc altogether: ENOENT. The caller checks
    // it is alive around the read; any other failure proves nothing.
    assert.ok(['EACCES', 'ENOENT'].includes(error.code), `unexpected ${error.code}`);
    return;
  }
  throw new Error('another runner process environment is still readable');
}

async function main(want) {
  assert.ok(['before', 'after'].includes(want), 'expected before or after');
  const state = join(process.env.RUNNER_TEMP, 'secure-host-read-probe.pid');
  if (want === 'before') {
    // Bounded even on failed jobs. Keep RUNNER_TRACKING_ID for runner cleanup.
    const child = spawn('/usr/bin/sleep', ['600'], {
      detached: true,
      stdio: 'ignore',
      env: { ...process.env, SECURE_HOST_READ_PROBE: 'not-a-secret' },
    });
    await once(child, 'spawn');
    child.unref();
    writeFileSync(state, String(child.pid));
    assert.ok(readFileSync(`/proc/${child.pid}/environ`).toString().split('\0').includes(canary));
  } else {
    const pid = Number(readFileSync(state, 'utf8'));
    assert.ok(Number.isSafeInteger(pid) && pid > 1, 'invalid probe PID');
    process.kill(pid, 0);
    refused(() => readFileSync(`/proc/${pid}/environ`));
    process.kill(pid, 0);
    // Leave it alive: the job's normal cleanup must work with the rule on.
  }
  console.log(`ok: ${want} runner environment read probe`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main(process.argv[2]);
}
