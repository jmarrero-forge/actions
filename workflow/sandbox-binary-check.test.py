"""Local tests of the runner probe; these do not prove host confinement."""

import contextlib
import importlib.util
import io
import os
from pathlib import Path
import subprocess
from types import SimpleNamespace
import unittest
from unittest.mock import mock_open, patch

spec = importlib.util.spec_from_file_location(
    "probe", Path(__file__).with_name("sandbox-binary-check.py")
)
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class ProbeTests(unittest.TestCase):
    def run_probe(self, status=0, output=b"agentic-job 0.6.4\n", uid=None):
        uid = os.getuid() + 1 if uid is None else uid
        user = SimpleNamespace(pw_uid=uid, pw_dir="/home/runner-sandbox")

        def run(command, **kwargs):
            self.assertEqual(command[:7], [
                "sudo", "-n", "--", "/usr/local/libexec/agentic-job",
                "helper", "enter", "--chdir",
            ])
            self.assertEqual(command[-1], str(uid))
            self.assertIn('exec agentic-job --version', command[-3])
            self.assertEqual(kwargs["timeout"], 60)
            self.assertIs(kwargs["stdin"], kwargs["stdout"])
            self.assertIs(kwargs["stderr"], kwargs["stdout"])
            self.assertEqual(kwargs["stdin"].recv(1), b"")
            kwargs["stdout"].sendall(output)
            return subprocess.CompletedProcess(command, status)

        with patch("builtins.open", mock_open(read_data=b"")), \
                patch.object(probe.pwd, "getpwnam", return_value=user), \
                patch.object(probe.subprocess, "check_output", return_value="agentic-job 0.6.4\n") as version, \
                patch.object(probe.subprocess, "run", side_effect=run), \
                contextlib.redirect_stdout(io.StringIO()):
            probe.main()
            version.assert_called_once_with(
                ["/var/tmp/agentic-job/agentic-job", "--version"], timeout=10, text=True
            )

    def test_success(self):
        self.run_probe()

    def test_refused_execution(self):
        with self.assertRaisesRegex(SystemExit, "exit 1"):
            self.run_probe(status=1, output=b"permission denied\n")

    def test_wrong_binary(self):
        with self.assertRaisesRegex(SystemExit, "does not match policy binary"):
            self.run_probe(output=b"agentic-job 0.6.3\n")

    def test_version_must_match_exactly(self):
        with self.assertRaisesRegex(SystemExit, "does not match policy binary"):
            self.run_probe(output=b"unexpected output\nagentic-job 0.6.4\n")

    def test_output_bound(self):
        with self.assertRaisesRegex(SystemExit, "exceeded 4096"):
            self.run_probe(output=b"x" * 4097)

    def test_wrong_user(self):
        for uid in (0, os.getuid()):
            with self.assertRaisesRegex(SystemExit, "separate, unprivileged"):
                self.run_probe(uid=uid)


if __name__ == "__main__":
    unittest.main()
