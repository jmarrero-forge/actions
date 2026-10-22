import { appendFileSync, readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

// Containerfile.pin is one FROM line, so that Dependabot's docker updater
// moves its tag and digest together. Only a digest is run: a tag alone
// would let whoever can push to the package choose what runs as root.
const FROM = /^FROM ([a-z0-9.-]+(?::[0-9]+)?(?:\/[a-z0-9._-]+)+)(?::[A-Za-z0-9_.-]{1,128})?@(sha256:[0-9a-f]{64})$/;

// Who must have built the image: the attestation's signer, checked by
// `gh attestation verify` against the digest before anything is pulled.
const SIGNER = {
  repository: /^[A-Za-z0-9-]+\/[A-Za-z0-9._-]+$/,
  workflow: /^\.github\/workflows\/[A-Za-z0-9._-]+\.ya?ml$/,
  ref: /^refs\/(?:heads|tags)\/[A-Za-z0-9._\/-]+$/,
};

export function pinned(pinText, signer) {
  const lines = pinText.split('\n').filter(line => line.trim() !== '');
  if (lines.length !== 1) throw new Error('Containerfile.pin must hold exactly one FROM line');
  const from = FROM.exec(lines[0]);
  if (!from) throw new Error(`Containerfile.pin is not FROM NAME[:TAG]@sha256:DIGEST: ${JSON.stringify(lines[0])}`);
  for (const [key, pattern] of Object.entries(SIGNER)) {
    if (typeof signer[key] !== 'string' || !pattern.test(signer[key])) {
      throw new Error(`signer.json: ${key} is missing or malformed`);
    }
  }
  return {
    image: `${from[1]}@${from[2]}`,
    repository: signer.repository,
    'cert-identity': `https://github.com/${signer.repository}/${signer.workflow}@${signer.ref}`,
    'source-ref': signer.ref,
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const result = pinned(
    readFileSync(new URL('./Containerfile.pin', import.meta.url), 'utf8'),
    JSON.parse(readFileSync(new URL('./signer.json', import.meta.url), 'utf8')),
  );
  appendFileSync(process.env.GITHUB_OUTPUT,
    Object.entries(result).map(([key, value]) => `${key}=${value}\n`).join(''));
}
