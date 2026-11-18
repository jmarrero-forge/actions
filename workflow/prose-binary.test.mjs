// Optional offline release-contract test:
// AGENTIC_JOB_PROSE_BINARY=/path/to/verified/v0.6.12 node --test workflow/prose-binary.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { task } from './prose.mjs';

const binary = process.env.AGENTIC_JOB_PROSE_BINARY;
test('v0.6.12 policy/collect/check exact advisory schema, noop, bounds and forged destination',
  { skip: !binary && 'set AGENTIC_JOB_PROSE_BINARY to the verified v0.6.12 binary' }, () => {
    const temp = mkdtempSync(join(tmpdir(), 'prose-contract-'));
    const run = args => spawnSync(binary, args, { encoding: 'utf8', timeout: 10000, maxBuffer: 2 * 1024 * 1024 });
    try {
      assert.match(run(['--version']).stdout, /0\.6\.12/);
      // The prose job's host configuration puts the policy binary's copy
      // first on the agent's PATH; v0.6.12 has no `[host]` table.
      const path = 'sandbox.env.PATH=/tmp/agentic-job-bin:/usr/local/bin:/usr/bin:/bin';
      const host = run(['config', '--host', '--string', path]);
      assert.equal(host.status, 0, host.stderr);
      assert.match(host.stdout, /PATH = "\/tmp\/agentic-job-bin:/);
      assert.notEqual(run(['config', '--host', '--string', path.replace('sandbox', 'host')]).status, 0);
      const compiled = run(['policy', '--allow', resolve('workflow/review.toml'), '--repo', 'jmarrero-forge/actions',
        '--clone-url', 'https://github.com/jmarrero-forge/actions', '--base', 'main', '--kind', 'analysis',
        '--outputs', 'add_comment,noop', '--max-outputs', '1', '--advisory-item', '195', '--advisory-head', 'a'.repeat(40)]);
      assert.equal(compiled.status, 0, compiled.stderr);
      const policy = JSON.parse(compiled.stdout);
      assert.equal(policy.advisory.head, 'a'.repeat(40));
      assert.deepEqual(policy.native_handlers, ['add_comment']);
      writeFileSync(join(temp, 'policy.json'), compiled.stdout);
      const outputs = join(temp, 'outputs');
      mkdirSync(outputs);
      const event = { action: 'created', repository: { full_name: 'jmarrero-forge/actions', private: false },
        issue: { number: 195, title: 'PR title', body: 'UNBOUNDED_BODY'.repeat(4000), pull_request: {} },
        comment: { id: 1, body: '/review COMMAND_NOT_INPUT', user: { login: 'owner', type: 'User' } },
        sender: { login: 'owner', type: 'User' } };
      writeFileSync(join(temp, 'event.json'), JSON.stringify(event));
      writeFileSync(join(temp, 'permission.json'), JSON.stringify({ permission: 'write', user: { login: 'owner' } }));
      const rendered = run(['event', '--allow', resolve('workflow/review-command.toml'), '--event-name', 'issue_comment',
        '--event', join(temp, 'event.json'), '--actor', 'owner', '--repository', 'jmarrero-forge/actions',
        '--actor-permission', join(temp, 'permission.json'), '--task', resolve('workflow/prose.md'), '--out', join(temp, 'event')]);
      assert.equal(rendered.status, 0, rendered.stderr);
      const eventTask = readFileSync(join(temp, 'event/task.md'), 'utf8');
      assert.match(eventTask, /COMMAND_NOT_INPUT/);
      // The prepared policy event task is deliberately discarded. Only the
      // base instruction, pinned documents, target and bounded data survive.
      const final = task(readFileSync('workflow/prose.md', 'utf8'), policy, { body: 'Bounded body', coverage: {} }, 'Trusted checklist');
      assert.doesNotMatch(final, /COMMAND_NOT_INPUT|UNBOUNDED_BODY|<agentic-job-event>/);
      const comment = { type: 'add_comment', repo: 'jmarrero-forge/actions', item_number: 195, body: 'Advisory prose only.' };
      const noop = { type: 'noop', message: 'No useful rewrite.' };
      for (const [items, ok] of [
        [[comment], true], [[noop], true], [[], false], [[comment, noop], false],
        [[{ ...comment, repo: 'other/repo' }], false], [[{ ...comment, item_number: 196 }], false],
        [[{ ...comment, head: 'a'.repeat(40) }], false], [[{ ...comment, body: 'x'.repeat(46001) }], false],
      ]) {
        writeFileSync(join(outputs, 'outputs.jsonl'), items.map(i => JSON.stringify(i)).join('\n') + '\n');
        const collected = join(temp, 'collected.json');
        const collect = run(['collect', '--policy', join(temp, 'policy.json'), '--outputs', outputs, '--collected', collected]);
        assert.ok([0, 1].includes(collect.status), collect.stderr);
        const checked = run(['check', '--policy', join(temp, 'policy.json'), '--outputs', outputs, '--collected', collected,
          '--report', join(temp, 'report.json'), '--comment-repo', 'jmarrero-forge/actions', '--comment-target', '195']);
        assert.equal(checked.status, ok ? 0 : 1, `${JSON.stringify(items).slice(0, 150)}\n${checked.stdout}\n${checked.stderr}`);
        assert.equal(JSON.parse(readFileSync(join(temp, 'report.json'), 'utf8')).ok, ok);
      }
    } finally {
      rmSync(temp, { recursive: true, force: true });
    }
  });
