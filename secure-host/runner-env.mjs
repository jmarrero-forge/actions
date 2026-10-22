import { appendFileSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// What the image's runner.env may set for later steps: trust stores, as
// paths under the proxy's root-owned directory. Anything else in the file is
// refused rather than passed on, so GITHUB_ENV gains no other variable.
const KEYS = ['NODE_EXTRA_CA_CERTS', 'REQUESTS_CA_BUNDLE', 'SSL_CERT_FILE'];
const VALUE = /^\/etc\/egress-proxy\/[A-Za-z0-9._-]+$/;

export function runnerEnv(text) {
  const lines = [];
  for (const line of text.split('\n')) {
    if (!line.trim() || line.startsWith('#')) continue;
    const [, key, value] = line.match(/^([A-Z_]+)=(.*)$/) || [];
    if (!KEYS.includes(key) || !VALUE.test(value) || value.split('/').includes('..')) {
      throw new Error(`runner.env: refused line ${JSON.stringify(line)}`);
    }
    lines.push(`${key}=${value}\n`);
  }
  if (!lines.length) throw new Error('runner.env: no trust store to export');
  return lines.join('');
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  appendFileSync(process.env.GITHUB_ENV, runnerEnv(readFileSync('/etc/egress-proxy/runner.env', 'utf8')));
}
