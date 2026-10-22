import { createHash, createSign, randomUUID } from 'node:crypto';
import { readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// Installation tokens last an hour (tracker#766). A token with less than
// this left is minted again rather than handed to a call that may outlast it.
export const MARGIN_MS = 5 * 60 * 1000;

function base64url(value) {
  return Buffer.from(typeof value === 'string' ? value : JSON.stringify(value)).toString('base64url');
}

// The app's own JWT, which only mints. GitHub refuses one that lasts more
// than 10 minutes; iat is a minute back for clock skew, as GitHub suggests.
export function appJwt(appId, privateKey, now = Date.now()) {
  const iat = Math.floor(now / 1000) - 60;
  const body = `${base64url({ alg: 'RS256', typ: 'JWT' })}.${base64url({ iat, exp: iat + 540, iss: String(appId) })}`;
  return `${body}.${createSign('RSA-SHA256').update(body).sign(privateKey, 'base64url')}`;
}

export function repositories(text) {
  return (text || '').split(/[\n,]+/).map(name => name.trim()).filter(Boolean);
}

// The action's permission inputs, as PERMISSION_CONTENTS=write and so on,
// so that a token minted again has no more than the step's first one.
export const PERMISSIONS = ['actions', 'checks', 'contents', 'issues', 'pull_requests', 'statuses', 'workflows'];

export function permissions(env) {
  const granted = {};
  for (const name of PERMISSIONS) {
    const value = env[`PERMISSION_${name.toUpperCase()}`];
    if (value) granted[name] = value;
  }
  return granted;
}

async function json(response, what) {
  if (!response.ok) throw new Error(`${what} failed: HTTP ${response.status}`);
  return response.json();
}

// A token for the installation on OWNER, scoped to REPOSITORIES and the
// PERMISSION_* given, as the action's create-github-app-token step mints it.
export async function mint(env, request = fetch, now = Date.now()) {
  const repos = repositories(env.REPOSITORIES);
  const granted = permissions(env);
  if (!env.APP_ID || !env.PRIVATE_KEY || !env.OWNER || !repos.length || !Object.keys(granted).length) {
    throw new Error('APP_ID, PRIVATE_KEY, OWNER, REPOSITORIES and a PERMISSION_* are required');
  }
  const api = env.GITHUB_API_URL || 'https://api.github.com';
  const headers = {
    Accept: 'application/vnd.github+json',
    // A key stored on one line with literal \n works upstream, so here too.
    Authorization: `Bearer ${appJwt(env.APP_ID, env.PRIVATE_KEY.replaceAll('\\n', '\n'), now)}`,
    'X-GitHub-Api-Version': '2022-11-28',
  };
  const path = `${encodeURIComponent(env.OWNER)}/${encodeURIComponent(repos[0])}`;
  const installation = await json(await request(`${api}/repos/${path}/installation`, {
    headers, signal: AbortSignal.timeout(30000),
  }), 'Installation lookup');
  const token = await json(await request(`${api}/app/installations/${installation.id}/access_tokens`, {
    method: 'POST', headers, body: JSON.stringify({ repositories: repos, permissions: granted }),
    signal: AbortSignal.timeout(30000),
  }), 'Token request');
  if (typeof token.token !== 'string' || !token.token || Number.isNaN(Date.parse(token.expires_at))) {
    throw new Error('Token response did not include a token and its expiry');
  }
  return { token: token.token, expires_at: token.expires_at };
}

// Where the last token is kept between calls of one job, the runner's
// temporary directory being the job's own; one file per app and scope.
export function statePath(env) {
  if (env.APP_TOKEN_STATE) return env.APP_TOKEN_STATE;
  if (!env.RUNNER_TEMP) return null;
  const scope = [env.APP_ID, env.OWNER, repositories(env.REPOSITORIES).join(','), JSON.stringify(permissions(env))];
  const key = createHash('sha256').update(scope.join('\n'));
  return join(env.RUNNER_TEMP, `app-token-${key.digest('hex').slice(0, 16)}.json`);
}

// The kept token while it has more than MARGIN_MS left, else a new one.
// `unauthorized` is the caller's word that a call just returned 401 with
// the kept token (revoked, say): that one is dropped and a new one minted.
export async function token(env, { unauthorized = false } = {}, request = fetch, now = Date.now()) {
  const path = statePath(env);
  if (path && !unauthorized) {
    try {
      const kept = JSON.parse(readFileSync(path, 'utf8'));
      if (Date.parse(kept.expires_at) - now > MARGIN_MS) return kept.token;
    } catch {
      // No token kept yet, or an unreadable one: mint.
    }
  }
  const minted = await mint(env, request, now);
  if (path) {
    // A new file renamed over the old, so that calls at once each end with
    // a whole file and the last one's token. The token replaced is left to
    // expire: it had MARGIN_MS at most, or already failed with a 401.
    const temporary = `${path}.${randomUUID()}`;
    writeFileSync(temporary, JSON.stringify(minted), { mode: 0o600, flag: 'wx' });
    renameSync(temporary, path);
  }
  return minted.token;
}

// The end of the step: the kept token is removed and revoked, since every
// later step of the job runs as the same user and could read the file, and
// only the action's own token is revoked by upstream at the end of the job.
// A token already expired or revoked answers 401, which is as good. The
// kept token is first renamed aside, where token() no longer reuses it, and
// stays there until its revocation succeeds, so that a failed one (a
// timeout, a 5xx) is tried again by the next --revoke.
export async function revoke(env, request = fetch) {
  const path = statePath(env);
  if (!path) return;
  const pending = `${path}.revoking`;
  await revokeKept(pending, env, request);
  try {
    renameSync(path, pending);
  } catch (error) {
    if (error.code === 'ENOENT') return;
    throw error;
  }
  await revokeKept(pending, env, request);
}

async function revokeKept(file, env, request) {
  let kept;
  try {
    kept = JSON.parse(readFileSync(file, 'utf8'));
  } catch (error) {
    if (error.code === 'ENOENT') return;
    // Nothing readable to revoke.
  }
  if (typeof kept?.token === 'string') {
    const response = await request(`${env.GITHUB_API_URL || 'https://api.github.com'}/installation/token`, {
      method: 'DELETE', signal: AbortSignal.timeout(30000),
      headers: { Accept: 'application/vnd.github+json', Authorization: `Bearer ${kept.token}`, 'X-GitHub-Api-Version': '2022-11-28' },
    });
    if (!response.ok && response.status !== 401) throw new Error(`Token revocation failed: HTTP ${response.status}`);
  }
  rmSync(file, { force: true });
}

// GH_TOKEN=$(node remint.mjs) before each call or batch of calls; again
// with --unauthorized, once, after a call returned 401; --revoke when the
// step ends (README). The mask goes to stderr, which the runner reads for
// workflow commands too, so that stdout is the token alone.
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [arg, ...rest] = process.argv.slice(2);
  if (rest.length || ![undefined, '--unauthorized', '--revoke'].includes(arg)) {
    console.error('usage: remint.mjs [--unauthorized | --revoke]');
    process.exit(2);
  }
  if (arg === '--revoke') {
    await revoke(process.env);
  } else {
    const value = await token(process.env, { unauthorized: arg === '--unauthorized' });
    process.stderr.write(`::add-mask::${value}\n`);
    process.stdout.write(`${value}\n`);
  }
}
