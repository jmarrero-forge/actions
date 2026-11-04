# Export only validated trust-store paths, and nothing if any line is refused.
set -eo pipefail
export LC_ALL=C
value='^/etc/egress-proxy/[A-Za-z0-9._-]+$'
out=
while IFS= read -r line || [ -n "$line" ]; do
  if [[ "$line" =~ ^[[:space:]]*$ || "$line" == '#'* ]]; then continue; fi
  case "${line%%=*}" in NODE_EXTRA_CA_CERTS | REQUESTS_CA_BUNDLE | SSL_CERT_FILE) ok=yes ;; *) ok=no ;; esac
  if [ "$ok" != yes ] || ! [[ "${line#*=}" =~ $value && "${line#*=}/" != */../* ]]; then
    echo "::error::runner.env: refused line $line"; exit 1
  fi
  out+="$line"$'\n'
done < "$PROXY_ENV"
if [ -z "$out" ]; then echo "::error::runner.env: no trust store to export"; exit 1; fi
printf '%s' "$out" >> "$GITHUB_ENV"
