import { appendFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

// Mint the key a later job joins with when it has no identity token: the
// agent job, whose agent is the runner's user and could mint identity
// tokens for any audience with one. The key is what the Tailscale CLI
// makes from a federated login itself: single-use, ephemeral and
// preauthorized, with the tags of this job's join, and expiring. Spent by
// that job's join, it is worth nothing to what runs after it. Neither the
// identity token, the access token nor the key is printed.
export const KEY_EXPIRY_SECONDS = 3600;
const API = 'https://api.tailscale.com/api/v2';

async function json(response, what) {
  if (!response.ok) throw new Error(`${what} failed: HTTP ${response.status}`);
  return response.json();
}

export async function mint(env = process.env, request = fetch) {
  // With step debug logging on, the runner logs each output as it is set:
  // the key would be in this job's public log, unspent.
  if (env.RUNNER_DEBUG) {
    throw new Error('Refusing to mint a tailnet key with step debug logging on (RUNNER_DEBUG is set): the runner would log it as it sets the output; re-run without debug logging');
  }
  const tags = (env.TAGS ?? '').split(',').map(tag => tag.trim()).filter(Boolean);
  if (!tags.length) throw new Error('Minting a key needs the join\'s tags');
  const oidcURL = new URL(env.ACTIONS_ID_TOKEN_REQUEST_URL);
  oidcURL.searchParams.set('audience', env.AUDIENCE);
  const oidc = await json(await request(oidcURL, {
    headers: { Authorization: `Bearer ${env.ACTIONS_ID_TOKEN_REQUEST_TOKEN}` },
    signal: AbortSignal.timeout(30000),
  }), 'Identity request');
  if (typeof oidc.value !== 'string' || !oidc.value) throw new Error('OIDC response did not include a token');
  const token = await json(await request(`${API}/oauth/token-exchange`, {
    method: 'POST',
    body: new URLSearchParams({ client_id: env.OAUTH_CLIENT_ID, jwt: oidc.value }),
    signal: AbortSignal.timeout(30000),
  }), 'Token exchange');
  if (typeof token.access_token !== 'string' || !token.access_token) {
    throw new Error('Token response did not include an access token');
  }
  const create = { reusable: false, ephemeral: true, preauthorized: true, tags };
  const created = await json(await request(`${API}/tailnet/-/keys`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${token.access_token}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ capabilities: { devices: { create } }, expirySeconds: KEY_EXPIRY_SECONDS }),
    redirect: 'error',
    signal: AbortSignal.timeout(30000),
  }), 'Key creation');
  // A key with a newline would write a second output line.
  if (typeof created.key !== 'string' || !/^tskey-[A-Za-z0-9_-]+$/.test(created.key)) {
    throw new Error('Tailscale returned no auth key');
  }
  return created.key;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const key = await mint();
  // Not masked: GitHub drops a job output holding a masked value. The
  // caller hands it on only as a secret of a reusable workflow call.
  appendFileSync(process.env.GITHUB_OUTPUT, `key=${key}\n`);
  console.log(`Minted a single-use, ephemeral key, valid for ${KEY_EXPIRY_SECONDS / 60} minutes`);
}
