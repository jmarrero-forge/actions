//! Exercise the shipped Node probe; only the test orchestration moves to Rust.
//! Linux /proc and Node are required, but no root, credentials or network.
#![cfg(target_os = "linux")]

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;

const LIMIT: Duration = Duration::from_secs(5);

// This bridge only calls the production API with fault-injection options.
// All assertions, deadlines and cleanup belong to the Rust harness.
const BRIDGE: &str = r#"
import { startProbe } from './secure-host/read-probe.mjs';
import { writeFileSync, existsSync } from 'node:fs';
const pid = await startProbe(process.env.PROBE_PID_FILE, JSON.parse(process.env.PROBE_OPTIONS));
writeFileSync(process.env.COMPLETE_FILE, JSON.stringify({pid}));
// Keep the launcher alive while Rust inspects /proc, as the original in-process
// tests did. Some test runners reap descendants when their launcher exits.
const deadline = setTimeout(() => process.exit(2), 10000);
const poll = setInterval(() => {
  if (existsSync(process.env.FINISH_FILE)) {
    clearTimeout(deadline);
    clearInterval(poll);
  }
}, 10);
"#;

struct Probe {
    child: Child,
    dir: tempfile::TempDir,
}

impl Probe {
    fn start(code: Option<&str>, canary: bool, timeout_ms: u64) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut options = json!({"timeoutMs": timeout_ms});
        if let Some(code) = code {
            // Record the actual PID even for failed startup, so Drop cleans up
            // a broken implementation that forgets to terminate its child.
            options["args"] = json!([
                "-e",
                format!(
                    "require('node:fs').writeFileSync(process.env.CHILD_PID_FILE, String(process.pid)); {code}"
                )
            ]);
        }
        let mut command = Command::new("node");
        command
            .args(["--input-type=module", "-e", BRIDGE])
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())
            .env("PROBE_PID_FILE", dir.path().join("probe.pid"))
            .env("CHILD_PID_FILE", dir.path().join("child.pid"))
            .env("RELEASE_FILE", dir.path().join("release"))
            .env("COMPLETE_FILE", dir.path().join("complete"))
            .env("FINISH_FILE", dir.path().join("finish"))
            .env("PROBE_OPTIONS", options.to_string())
            .env("RUNNER_TRACKING_ID", "read-probe-test")
            .env_remove("SECURE_HOST_READ_PROBE")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(dir.path().join("stderr")).unwrap());
        if canary {
            command.env("SECURE_HOST_READ_PROBE", "not-a-secret");
        }
        Self {
            child: command.spawn().expect("Node must be on PATH"),
            dir,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn wait(&mut self, limit: Duration) -> ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(start.elapsed() < limit, "probe launcher did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn stderr(&self) -> String {
        fs::read_to_string(self.path("stderr")).unwrap()
    }

    fn ready(&mut self) {
        let start = Instant::now();
        while !self.path("complete").exists() {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "{}",
                self.stderr()
            );
            assert!(start.elapsed() < LIMIT, "probe never became ready");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn finish(&mut self) {
        fs::write(self.path("finish"), "done").unwrap();
        assert!(self.wait(LIMIT).success(), "{}", self.stderr());
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for name in ["probe.pid", "child.pid"] {
            if let Ok(text) = fs::read_to_string(self.path(name))
                && let Ok(pid) = text.trim().parse::<u32>()
                && pid > 1
            {
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }
}

#[test]
fn detached_probe_retains_exact_pid_environment_and_session() {
    let mut probe = Probe::start(None, true, 5000);
    probe.ready();
    let text = fs::read_to_string(probe.path("probe.pid")).unwrap();
    let pid: u32 = text.trim().parse().unwrap();
    assert_eq!(text, format!("{pid}\n"));
    let returned: serde_json::Value =
        serde_json::from_slice(&fs::read(probe.path("complete")).unwrap()).unwrap();
    assert_eq!(
        returned["pid"],
        json!(pid),
        "API return must match published PID"
    );
    let environ = fs::read(format!("/proc/{pid}/environ")).unwrap();
    for expected in [
        b"SECURE_HOST_READ_PROBE=not-a-secret".as_slice(),
        b"RUNNER_TRACKING_ID=read-probe-test".as_slice(),
    ] {
        assert!(environ.split(|b| *b == 0).any(|entry| entry == expected));
    }
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    // comm may contain spaces: fields after its closing parenthesis start at 3.
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    assert_eq!(fields[3].parse::<u32>().unwrap(), pid, "session leader");
    probe.finish();
}

#[test]
fn publication_waits_for_readiness_not_just_a_live_pid_and_canary() {
    let mut probe = Probe::start(
        Some(
            r#"
setTimeout(() => process.exit(0), 10000);
const timer = setInterval(() => {
  if (require('node:fs').existsSync(process.env.RELEASE_FILE)) {
    clearInterval(timer);
    process.send({pid: process.pid});
  }
}, 20);
"#,
        ),
        true,
        5000,
    );
    let start = Instant::now();
    while !probe.path("child.pid").exists() {
        assert!(
            probe.child.try_wait().unwrap().is_none(),
            "{}",
            probe.stderr()
        );
        assert!(start.elapsed() < LIMIT, "child did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(!probe.path("probe.pid").exists());
    assert!(probe.child.try_wait().unwrap().is_none());
    fs::write(probe.path("release"), "ready").unwrap();
    probe.ready();
    let actual = fs::read_to_string(probe.path("child.pid")).unwrap();
    assert_eq!(
        fs::read_to_string(probe.path("probe.pid")).unwrap(),
        format!("{actual}\n")
    );
    probe.finish();
}

#[test]
fn startup_failures_never_publish_a_pid_or_leave_the_launcher_hanging() {
    for (name, code, canary, timeout_ms, errors) in [
        (
            "timeout",
            "setTimeout(() => {}, 10000)",
            true,
            100,
            vec!["timed out"],
        ),
        (
            "early exit",
            "process.exit(1)",
            true,
            5000,
            vec!["exited", "disconnected"],
        ),
        (
            "wrong PID",
            "process.send({pid: process.pid + 1}); setTimeout(() => {}, 10000)",
            true,
            5000,
            vec!["PID mismatch"],
        ),
        (
            "missing canary",
            "process.send({pid: process.pid}); setTimeout(() => {}, 10000)",
            false,
            5000,
            vec!["missing the canary"],
        ),
    ] {
        let mut probe = Probe::start(Some(code), canary, timeout_ms);
        // In particular, timeout must exit with code 1, not stay on IPC until
        // killed by the test's deadline (the old separate subprocess test).
        assert_eq!(probe.wait(Duration::from_secs(3)).code(), Some(1), "{name}");
        assert!(!probe.path("probe.pid").exists(), "{name}");
        let stderr = probe.stderr();
        assert!(
            errors.iter().any(|error| stderr.contains(error)),
            "{name}: {stderr}"
        );
    }
}
