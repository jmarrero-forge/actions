"""Check the wrappers' binary as the image's sandbox user, after hardening.

v0.6.4 has no `sandbox exec`. Use the same narrow sudo helper as its
supervisor, with socket standard streams for run0's handoff to systemd.
Python's socketpair is needed here; Node's standard library has none.
"""

import os
import pwd
import socket
import subprocess
import tomllib


def main():
    expected = subprocess.check_output(
        ["/var/tmp/agentic-job/agentic-job", "--version"], timeout=10, text=True
    ).strip()
    with open("/etc/agentic-job/config.toml", "rb") as stream:
        config = tomllib.load(stream)
    user = pwd.getpwnam(config.get("sandbox", {}).get("user", "runner-sandbox"))
    if user.pw_uid in (0, os.getuid()):
        raise SystemExit("expected a separate, unprivileged sandbox user")
    command = [
        "sudo", "-n", "--", "/usr/local/libexec/agentic-job", "helper", "enter",
        "--chdir", user.pw_dir, "--", "sh", "-ec",
        'test "$(id -u)" = "$1"; '
        'test ! -r /var/tmp/agentic-job/policy.json; '
        'test ! -w /var/tmp/agentic-job/agentic-job; '
        'exec agentic-job --version',
        "wrapper-smoke", str(user.pw_uid),
    ]
    parent, child = socket.socketpair()
    with parent, child:
        parent.shutdown(socket.SHUT_WR)
        # Fixed commands, tiny output. Both execution and reading are bounded;
        # a command unexpectedly filling the socket fails on the timeout.
        result = subprocess.run(command, stdin=child, stdout=child, stderr=child, timeout=60)
        child.close()
        parent.settimeout(5)
        data = bytearray()
        while chunk := parent.recv(4097 - len(data)):
            data.extend(chunk)
            if len(data) > 4096:
                raise SystemExit("sandbox binary check output exceeded 4096 bytes")
        output = data.decode("utf-8", errors="replace")
    print(output, end="")
    if result.returncode != 0:
        raise SystemExit(f"sandbox binary check failed: exit {result.returncode}")
    if output.strip() != expected:
        raise SystemExit(f"sandbox CLI version does not match policy binary: expected {expected!r}")


if __name__ == "__main__":
    main()
