import { copyFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { userInfo } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

export function configure(env, run = spawnSync) {
  const mode = env.EGRESS || 'off';
  if (!['off', 'write-policy'].includes(mode)) throw new Error('egress must be off or write-policy');
  if (env.EGRESS_POLICY && mode !== 'write-policy') throw new Error('egress-policy needs egress: write-policy');
  const dir = resolve(env.RUNNER_TEMP, 'secure-host');
  mkdirSync(join(dir, 'in'), { recursive: true });
  copyFileSync(env.CONFIG || join(env.ACTION, 'hosted.toml'), join(dir, 'in/config.toml'));
  const settings = ['--string', `sandbox.runner-user=${userInfo().username}`,
    '--boolean', `egress.write-policy=${mode === 'write-policy'}`];
  if (mode === 'write-policy') {
    // Stop a later step POSTing to an arbitrary destination. Reads and the
    // runner's built-in write endpoints remain open; this is not a data diode.
    settings.push('--boolean', 'egress.proxy=true', '--boolean', 'egress.rust-proxy=true');
    if (env.EGRESS_POLICY) {
      // Setup reads this on the host, not inside the configuration container.
      const policy = join(dir, 'policy.toml');
      copyFileSync(env.EGRESS_POLICY, policy);
      settings.push('--string', `egress.policy=${policy}`);
    }
  }
  const result = run('sudo', ['podman', 'run', '--rm', '--network=none', '--entrypoint', '/agentic-job',
    '-v', `${dir}/in:/in:ro`, env.ID, 'config', '--host', '--from', '/in/config.toml', ...settings],
  { encoding: 'utf8' });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`host configuration refused: ${result.stderr}`);
  writeFileSync(join(dir, 'config.toml'), result.stdout);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  configure(process.env);
}
