# The shared workflow check (lint/), over this repository.
lint:
    {{ just_executable() }} --justfile lint/Justfile --working-directory . check

# The check's unit tests.
test-lint:
    {{ just_executable() }} --justfile lint/Justfile test

# Reusable event-review admission and credential-boundary regressions.
test-review:
    node --test workflow/review.test.cjs

# The wrappers' owner allowlist.
test-admit:
    node --test admit/*.test.mjs

# secure-host's pin and signer checks.
test-secure-host:
    node --test secure-host/*.test.mjs

# The proxy, not an origin error or timeout, must refuse the write; and
# with ALLOWED, a path of example.com an `egress-policy` rule allows, let
# that POST through.
secure-host-egress allowed="":
    node secure-host/egress-probe.mjs {{ quote(allowed) }}

# A readable invalid policy must fail setup, not only sandbox check.
secure-host-invalid-policy:
    test "$OUTCOME" = failure

# The secure-host workflow's host checks (secure-host/README.md), each run
# by the runner's user in a step of its own. WANT says which side of the
# action the step is on: `before` for the controls, `after` for the claims.

# No root and no docker after the action; both before it.
secure-host-root want:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "{{ want }}" = before ]; then
      sudo -n true && systemctl is-active --quiet docker.service
      exit
    fi
    if sudo -n true 2> /dev/null; then echo "::error::the runner's user still has sudo"; exit 1; fi
    if systemctl is-active --quiet docker.service; then echo "::error::docker is still running"; exit 1; fi

# The `config` input: `before` writes a configuration that keeps the
# runner's root; `after`, that was the one taken, though docker still stopped.
secure-host-options want:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "{{ want }}" = before ]; then
      sed -e 's/^lock-runner = true$/lock-runner = false/' -e 's/^deny-runner-metadata = true$/deny-runner-metadata = false/' -e 's/^deny-runner-reads = true$/deny-runner-reads = false/' \
        secure-host/hosted.toml > "$RUNNER_TEMP/unlocked.toml"
      grep -qx 'lock-runner = false' "$RUNNER_TEMP/unlocked.toml"
      exit
    fi
    sudo -n true
    if systemctl is-active --quiet docker.service; then echo "::error::docker is still running"; exit 1; fi

# The cloud metadata service (Azure's, on GitHub's hosted runners) answers
# before the action and not after it.
secure-host-metadata want:
    #!/usr/bin/env bash
    set -euo pipefail
    url="http://169.254.169.254/metadata/instance?api-version=2021-02-01"
    if curl -sS --fail --max-time 10 -H Metadata:true -o /dev/null "$url"; then answered=yes; else answered=no; fi
    case "{{ want }}-$answered" in
      before-yes | after-no) ;;
      *) echo "::error::{{ want }} the action, the metadata service answered: $answered"; exit 1 ;;
    esac
    # Control: the refusal is the metadata rule's, not a network that is down.
    curl -sS --fail --max-time 10 -o /dev/null https://github.com/

# A root-owned mode-0666 file on the root filesystem, which ordinary
# permissions let anyone append to: `before` makes it and appends,
# `after` must be refused by the world-write policy, while a mode-0666
# file of the runner's own on the root filesystem still takes the write.
secure-host-world-write want:
    #!/usr/bin/env bash
    set -euo pipefail
    probe=/var/lib/secure-host-ci/file
    if [ "{{ want }}" = before ]; then
      sudo install -d -m 0755 /var/lib/secure-host-ci
      sudo install -m 0666 /dev/null "$probe"
      test "$(stat -c '%u %a %d' "$probe")" = "0 666 $(stat -c %d /)"
      echo control >> "$probe"
      exit
    fi
    if (echo denied >> "$probe") 2> "$RUNNER_TEMP/refused.txt"; then
      echo "::error::the runner's user appended to $probe"; exit 1
    fi
    sed 's/::/: :/g' "$RUNNER_TEMP/refused.txt"
    grep -q "Permission denied" "$RUNNER_TEMP/refused.txt"
    owned="$HOME/secure-host-owned"
    install -m 0666 /dev/null "$owned"
    test "$(stat -c '%a %d' "$owned")" = "666 $(stat -c %d /)"
    echo control >> "$owned"
    grep -qx control "$owned"
