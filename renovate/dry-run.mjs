// Runs Renovate (on PATH; `just test-renovate` provides it) offline, with
// the local platform and runners.json as the repository's configuration,
// over a copy of this checkout plus the fixture workflow below, and fails
// unless the updates it would propose are exactly the fixture's stale
// Ubuntu labels: none for this repository's own action pins, images and
// crates, which Dependabot keeps, nor for the fixture's other runners.
import { execFileSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

// What consumers and this repository extend, renovate/runners.json here.
const PRESET = 'github>cgwalters-forge/actions//renovate/runners';

const FIXTURE = 'workflow-templates/runners.yml';

// Literal labels, the only form Renovate reads: a string or a list of
// strings. It skips an expression (a matrix, an input) and a group.
const WORKFLOW = `on: [push, workflow_call]
jobs:
  plain: {runs-on: ubuntu-24.04, steps: [{run: 'true'}]}
  older: {runs-on: ubuntu-22.04, steps: [{run: 'true'}]}
  arm: {runs-on: [ubuntu-24.04-arm], steps: [{run: 'true'}]}
  current: {runs-on: ubuntu-26.04, steps: [{run: 'true'}]}
  slim: {runs-on: ubuntu-slim, steps: [{run: 'true'}]}
  self-hosted: {runs-on: [self-hosted, linux, x64], steps: [{run: 'true'}]}
  group: {runs-on: {group: big, labels: [ubuntu-24.04-16core]}, steps: [{run: 'true'}]}
  mac: {runs-on: macos-14, steps: [{run: 'true'}]}
  windows: {runs-on: windows-2022, steps: [{run: 'true'}]}
  matrix:
    strategy: {matrix: {os: [ubuntu-24.04]}}
    runs-on: \${{ matrix.os }}
    steps: [{run: 'true'}]
`;

// What Renovate should propose, as `file: label -> label`.
const EXPECTED = [
  `${FIXTURE}: ubuntu-22.04 -> ubuntu-26.04`,
  `${FIXTURE}: ubuntu-24.04 -> ubuntu-26.04`,
  `${FIXTURE}: ubuntu-24.04-arm -> ubuntu-26.04-arm`,
];

const TIMEOUT_MS = 5 * 60 * 1000;

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const own = JSON.parse(readFileSync(join(root, '.github/renovate.json'), 'utf8'));
if (JSON.stringify(own.extends) !== JSON.stringify([PRESET])) {
  throw new Error(`.github/renovate.json extends ${JSON.stringify(own.extends)}, not ["${PRESET}"]`);
}
const work = mkdtempSync(join(tmpdir(), 'renovate-dry-run-'));
try {
  const files = execFileSync('git', ['ls-files', '-z', '--cached', '--others', '--exclude-standard'], {
    cwd: root, encoding: 'utf8',
  }).split('\0').filter(Boolean);
  for (const file of files) {
    try {
      cpSync(join(root, file), join(work, file), { recursive: true });
    } catch (e) {
      if (e.code !== 'ENOENT') throw e; // deleted in the working tree
    }
  }
  // The preset is what an `extends` of it inlines; this repository's own
  // configuration names it on GitHub, which an offline run cannot fetch.
  rmSync(join(work, '.github/renovate.json'), { force: true });
  cpSync(join(root, 'renovate/runners.json'), join(work, 'renovate.json'));
  mkdirSync(join(work, dirname(FIXTURE)), { recursive: true });
  writeFileSync(join(work, FIXTURE), WORKFLOW);

  const out = execFileSync('renovate', ['--platform=local', '--dry-run=lookup'], {
    cwd: work,
    encoding: 'utf8',
    timeout: TIMEOUT_MS,
    maxBuffer: 256 * 1024 * 1024,
    env: { ...process.env, LOG_LEVEL: 'debug', LOG_FORMAT: 'json', RENOVATE_CONFIG_FILE: '' },
  });
  const records = out.split('\n').flatMap((line) => {
    try { return [JSON.parse(line)]; } catch { return []; }
  });
  const found = records.find((r) => r.msg === 'packageFiles with updates');
  if (!found) throw new Error('Renovate logged no "packageFiles with updates"; its output:\n' + out.slice(-4000));

  const got = [];
  let deps = 0;
  for (const packageFiles of Object.values(found.config)) {
    for (const { packageFile, deps: list } of packageFiles) {
      for (const dep of list) {
        deps++;
        for (const update of dep.updates ?? []) {
          got.push(`${packageFile}: ${dep.replaceString ?? dep.depName} -> ${dep.depName}-${update.newValue}`);
        }
      }
    }
  }
  got.sort();
  if (deps < 10) throw new Error(`Renovate extracted only ${deps} dependencies; is this the whole checkout?`);
  if (JSON.stringify(got) !== JSON.stringify(EXPECTED)) {
    throw new Error(`Renovate would propose:\n  ${got.join('\n  ') || '(nothing)'}\nnot:\n  ${EXPECTED.join('\n  ')}`);
  }
  console.log(`${deps} dependencies extracted; updates, all expected:\n  ${got.join('\n  ')}`);
} finally {
  rmSync(work, { recursive: true, force: true });
}
