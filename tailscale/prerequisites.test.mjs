import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

// Execute the action's prerequisites step against stub os-release files with
// a recording sudo double: only RHEL 10 installs or links anything.
test('only RHEL 10 installs iptables and links the CLI into sudo\'s PATH', () => {
  const action = readFileSync(new URL('./action.yml', import.meta.url), 'utf8');
  const step = action.split('    - name: Install RHEL prerequisites\n')[1]
    .split('    - name: Connect to the tailnet\n')[0];
  assert.match(step, /OS_RELEASE: \/etc\/os-release\n/);
  assert.ok(action.indexOf('Install RHEL prerequisites') < action.indexOf('Connect to the tailnet'));
  const script = step.split('      run: |\n')[1]
    .split('\n').map(line => line.replace(/^        /, '')).join('\n');
  const directory = mkdtempSync(join(homedir(), 'tailscale-prerequisites-'));
  const log = join(directory, 'calls');
  const osRelease = join(directory, 'os-release');
  try {
    writeFileSync(join(directory, 'sudo'), `#!${process.execPath}
require('node:fs').appendFileSync(process.env.CALL_LOG, JSON.stringify(process.argv.slice(2)) + '\\n');
`, { mode: 0o700 });
    const rhel = [
      ['dnf', 'install', '-y', 'iptables'],
      ['ln', '-sfn', '/usr/local/bin/tailscale', '/usr/bin/tailscale'],
      ['ln', '-sfn', '/usr/local/bin/tailscaled', '/usr/bin/tailscaled'],
    ];
    for (const [release, expected] of [
      ['NAME="Red Hat Enterprise Linux"\nID="rhel"\nID_LIKE="centos fedora"\nVERSION_ID="10.0"\n', rhel],
      ['ID=rhel\nVERSION_ID=10\n', rhel],
      ['NAME="Ubuntu"\nID=ubuntu\nID_LIKE=debian\nVERSION_ID="24.04"\n', []],
      ['ID="rhel"\nVERSION_ID="9.6"\n', []],
      ['ID="rhel"\nVERSION_ID="100"\n', []],
      ['ID="centos"\nID_LIKE="rhel fedora"\nVERSION_ID="10"\n', []],
      ['', []],
      [null, []],
    ]) {
      if (release === null) rmSync(osRelease, { force: true });
      else writeFileSync(osRelease, release);
      writeFileSync(log, '');
      const result = spawnSync('bash', ['--noprofile', '--norc', '-e', '-o', 'pipefail', '-c', script], {
        encoding: 'utf8',
        env: { ...process.env, PATH: `${directory}:${process.env.PATH}`, CALL_LOG: log, OS_RELEASE: osRelease },
      });
      assert.ifError(result.error);
      assert.equal(result.status, 0, result.stderr);
      const calls = readFileSync(log, 'utf8').split('\n').filter(Boolean).map(line => JSON.parse(line));
      assert.deepEqual(calls, expected, release);
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
