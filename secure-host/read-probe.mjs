import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const filename = fileURLToPath(import.meta.url);
const canary = 'SECURE_HOST_READ_PROBE=not-a-secret';

// Prevent a launch/exec race from invalidating the positive control for reads
// of another runner process's environment. Readiness is not itself evidence:
// require the real /proc read of this exact child before publishing its PID.
export async function startProbe(pidFile, {
  args = [filename, '--hold'], timeoutMs = 5000, env = process.env,
} = {}) {
  const child = spawn(process.execPath, args, {
    detached: true, stdio: ['ignore', 'ignore', 'ignore', 'ipc'], env,
  });
  let timer;
  try {
    await new Promise((resolve, reject) => {
      timer = setTimeout(() => reject(new Error('probe readiness timed out')), timeoutMs);
      child.once('error', reject);
      child.once('exit', (code, signal) => {
        reject(new Error(`probe exited before readiness (${signal ?? code})`));
      });
      child.once('disconnect', () => reject(new Error('probe disconnected before readiness')));
      child.once('message', (message) => {
        if (message?.pid !== child.pid) reject(new Error('probe readiness PID mismatch'));
        else resolve();
      });
    });
    process.kill(child.pid, 0);
    assert.ok(readFileSync(`/proc/${child.pid}/environ`, 'utf8').split('\0').includes(canary),
      'probe environment is missing the canary');
    process.kill(child.pid, 0);
    writeFileSync(pidFile, `${child.pid}\n`);
    // Keep the original RUNNER_TRACKING_ID and leave the live child to normal
    // job orphan cleanup. Its own timer also bounds it on failed/cancelled jobs.
    child.disconnect();
    child.unref();
    return child.pid;
  } catch (error) {
    child.kill('SIGKILL');
    throw error;
  } finally {
    clearTimeout(timer);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  if (process.argv[2] === '--hold') {
    // Install the finite lifetime before acknowledging that this process has
    // finished starting. No exec or PID-changing launcher follows readiness.
    setTimeout(() => process.exit(0), 600_000);
    process.send({ pid: process.pid });
  } else {
    assert.ok(process.env.RUNNER_TEMP, 'RUNNER_TEMP must be set');
    await startProbe(join(process.env.RUNNER_TEMP, 'secure-host-read-probe.pid'));
  }
}
