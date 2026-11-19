//! Exercise the real Renovate engine, not a Rust approximation of its managers.
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

const PRESET: &str = "github>cgwalters-forge/actions//renovate/runners";
const FIXTURE: &str = "workflow-templates/runners.yml";
const TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAX_OUTPUT: u64 = 256 * 1024 * 1024;
const WORKFLOW: &str = r#"on: [push, workflow_call]
jobs:
  plain: {runs-on: ubuntu-24.04, steps: [{run: 'true'}]}
  older: {runs-on: ubuntu-22.04, steps: [{run: 'true'}]}
  arm: {runs-on: [ubuntu-24.04-arm], steps: [{run: 'true'}]}
  current: {runs-on: ubuntu-26.04, steps: [{run: 'true'}]}
  slim: {runs-on: ubuntu-slim, steps: [{run: 'true'}]}
  self-hosted: {runs-on: [self-hosted, linux, x64], steps: [{run: 'true'}]}
  group: {runs-on: {group: big, labels: [ubuntu-24.04-16core]}, steps: [{run: 'true'}]}
  mac: {runs-on: macos-14, steps: [{run: 'true'}]}
  windows: {runs-on: windows-2022, steps: [{run: 'true'}]}
  matrix:
    strategy: {matrix: {os: [ubuntu-24.04]}}
    runs-on: ${{ matrix.os }}
    steps: [{run: 'true'}]
"#;

fn check_preset(own: &Value) -> Result<()> {
    ensure!(
        own["extends"] == json!([PRESET]),
        ".github/renovate.json extends {}, not [\"{PRESET}\"]",
        own["extends"]
    );
    Ok(())
}

fn update_record(out: &str) -> Result<Value> {
    out.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| record["msg"] == "packageFiles with updates")
        .with_context(|| {
            let tail: String = out.chars().rev().take(4000).collect();
            format!(
                "Renovate logged no \"packageFiles with updates\"; its output:\n{}",
                tail.chars().rev().collect::<String>()
            )
        })
}

fn updates(record: &Value) -> Result<(usize, Vec<String>)> {
    let config = record["config"]
        .as_object()
        .context("missing config object")?;
    let mut deps = 0;
    let mut got = Vec::new();
    for files in config.values() {
        for file in files.as_array().context("package files must be an array")? {
            let name = file["packageFile"]
                .as_str()
                .context("missing packageFile")?;
            for dep in file["deps"].as_array().context("missing deps array")? {
                deps += 1;
                if dep["updates"].is_null() {
                    continue;
                }
                for update in dep["updates"].as_array().context("invalid updates array")? {
                    let dep_name = dep["depName"].as_str().context("missing depName")?;
                    let old = dep["replaceString"].as_str().unwrap_or(dep_name);
                    let new = update["newValue"].as_str().context("missing newValue")?;
                    got.push(format!("{name}: {old} -> {dep_name}-{new}"));
                }
            }
        }
    }
    got.sort();
    Ok((deps, got))
}

fn verify_updates(deps: usize, got: &[String]) -> Result<()> {
    ensure!(
        deps >= 10,
        "Renovate extracted only {deps} dependencies; is this the whole checkout?"
    );
    let expected = ["ubuntu-22.04", "ubuntu-24.04", "ubuntu-24.04-arm"].map(|old| {
        format!(
            "{FIXTURE}: {old} -> ubuntu-26.04{}",
            if old.ends_with("-arm") { "-arm" } else { "" }
        )
    });
    ensure!(
        got == expected,
        "Renovate would propose:\n  {}\nnot:\n  {}",
        if got.is_empty() {
            "(nothing)".into()
        } else {
            got.join("\n  ")
        },
        expected.join("\n  ")
    );
    Ok(())
}

// File-backed output avoids pipe deadlocks. Bound both logs while polling;
// kill and reap before returning, including on polling or metadata errors.
fn run_bounded(command: &mut Command, timeout: Duration, limit: u64) -> Result<String> {
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = command
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?))
        .spawn()
        .with_context(|| {
            format!(
                "could not start {:?}; is it on PATH?",
                command.get_program()
            )
        })?;
    let start = Instant::now();
    let result = (|| {
        loop {
            ensure!(
                stdout.metadata()?.len() <= limit && stderr.metadata()?.len() <= limit,
                "process output exceeded {limit} bytes"
            );
            if let Some(status) = child.try_wait()? {
                ensure!(status.success(), "process exited with {status}");
                return Ok(());
            }
            ensure!(
                start.elapsed() < timeout,
                "process timed out after {timeout:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    })();
    if let Err(error) = result {
        let _ = child.kill();
        let _ = child.wait();
        bail!(
            "{error:#}\nstdout tail:\n{}\nstderr tail:\n{}",
            output_tail(&mut stdout)?,
            output_tail(&mut stderr)?
        );
    }
    stdout.rewind()?;
    let mut out = String::new();
    stdout.take(limit + 1).read_to_string(&mut out)?;
    ensure!(
        out.len() as u64 <= limit,
        "process output exceeded {limit} bytes"
    );
    Ok(out)
}

fn output_tail(file: &mut File) -> Result<String> {
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(4000)))?;
    let mut bytes = Vec::new();
    file.take(4000).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn copy_checkout(root: &Path, work: &Path) -> Result<()> {
    let files = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .output()?;
    ensure!(
        files.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&files.stderr)
    );
    for file in files
        .stdout
        .split(|byte| *byte == 0)
        .filter(|file| !file.is_empty())
    {
        let file = std::str::from_utf8(file).context("non-UTF-8 checkout path")?;
        let source = root.join(file);
        let destination = work.join(file);
        fs::create_dir_all(destination.parent().context("file has no parent")?)?;
        if let Err(error) = fs::copy(&source, &destination)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error).with_context(|| format!("copying {file}"));
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires Renovate on PATH; run just test-renovate"]
fn renovate_dry_run() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    check_preset(&serde_json::from_slice(&fs::read(
        root.join(".github/renovate.json"),
    )?)?)?;
    let work = tempfile::tempdir()?;
    copy_checkout(root, work.path())?;
    fs::remove_file(work.path().join(".github/renovate.json"))?;
    fs::copy(
        root.join("renovate/runners.json"),
        work.path().join("renovate.json"),
    )?;
    fs::create_dir_all(work.path().join("workflow-templates"))?;
    fs::write(work.path().join(FIXTURE), WORKFLOW)?;
    let out = run_bounded(
        Command::new("renovate")
            .args(["--platform=local", "--dry-run=lookup"])
            .current_dir(work.path())
            .env("LOG_LEVEL", "debug")
            .env("LOG_FORMAT", "json")
            .env("RENOVATE_CONFIG_FILE", ""),
        TIMEOUT,
        MAX_OUTPUT,
    )?;
    let (deps, got) = updates(&update_record(&out)?)?;
    verify_updates(deps, &got)?;
    println!(
        "{deps} dependencies extracted; updates, all expected:\n  {}",
        got.join("\n  ")
    );
    Ok(())
}

#[test]
fn preset_contract() {
    for extends in [
        Value::Null,
        json!(PRESET),
        json!([]),
        json!([PRESET, "other"]),
    ] {
        assert!(check_preset(&json!({"extends": extends})).is_err());
    }
    check_preset(&json!({"extends": [PRESET], "customManagers": []})).unwrap();
}

#[test]
fn record_selection() {
    for prefix in ["", "not JSON\n", "null\n{}\n{\"msg\":\"other\"}\n"] {
        let record = update_record(&format!("{prefix}{{\"msg\":\"packageFiles with updates\",\"config\":{{}}}}\n{{\"msg\":\"packageFiles with updates\"}}" )).unwrap();
        assert_eq!(record["config"], json!({}));
    }
    let error = update_record(&format!("{}TAIL", "é".repeat(5000)))
        .unwrap_err()
        .to_string();
    assert!(error.contains("no \"packageFiles with updates\""));
    assert!(error.ends_with("TAIL"));
    assert!(!error.contains(&"é".repeat(4000)));
}

#[test]
fn update_formatting_and_validation() {
    let record = json!({"config": {"github-actions": [{"packageFile": FIXTURE, "deps": [
        {"depName": "ubuntu", "replaceString": "ubuntu-24.04", "updates": [{"newValue": "26.04"}]},
        {"depName": "ubuntu", "replaceString": "ubuntu-22.04", "updates": [{"newValue": "26.04"}]},
        {"depName": "ubuntu", "replaceString": "ubuntu-24.04-arm", "updates": [{"newValue": "26.04-arm"}]},
        {}, {"updates": null}, {"updates": []}, {}, {}, {}, {}
    ]}]}});
    let (deps, got) = updates(&record).unwrap();
    assert_eq!(deps, 10);
    verify_updates(deps, &got).unwrap();
    assert!(
        verify_updates(9, &got)
            .unwrap_err()
            .to_string()
            .contains("only 9")
    );
    assert!(
        verify_updates(10, &[])
            .unwrap_err()
            .to_string()
            .contains("(nothing)")
    );
    let (_, fallback) = updates(&json!({"config": {"other": [{"packageFile": "x", "deps": [{"depName": "a", "updates": [{"newValue": "2"}, {"newValue": "1"}]}]}]}})).unwrap();
    assert_eq!(fallback, ["x: a -> a-1", "x: a -> a-2"]);
    assert!(verify_updates(10, &fallback).is_err());
    for invalid in [
        json!({}),
        json!({"config": {"x": null}}),
        json!({"config": {"x": [{}]}}),
    ] {
        assert!(updates(&invalid).is_err());
    }
}

#[test]
fn checkout_copy_includes_untracked_but_not_ignored_or_deleted() -> Result<()> {
    let source = tempfile::tempdir()?;
    let destination = tempfile::tempdir()?;
    ensure!(
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(source.path())
            .status()?
            .success(),
        "git setup failed"
    );
    fs::write(source.path().join("tracked"), "tracked")?;
    fs::write(source.path().join("deleted"), "deleted")?;
    fs::write(source.path().join(".gitignore"), "ignored\n")?;
    ensure!(
        Command::new("git")
            .args(["add", "."])
            .current_dir(source.path())
            .status()?
            .success(),
        "git add failed"
    );
    fs::remove_file(source.path().join("deleted"))?;
    fs::create_dir(source.path().join("nested"))?;
    fs::write(
        source.path().join("nested/untracked with spaces"),
        "untracked",
    )?;
    fs::write(source.path().join("ignored"), "ignored")?;
    copy_checkout(source.path(), destination.path())?;
    for (path, expected) in [
        ("tracked", "tracked"),
        ("nested/untracked with spaces", "untracked"),
    ] {
        assert_eq!(fs::read_to_string(destination.path().join(path))?, expected);
    }
    for absent in ["deleted", "ignored", ".git"] {
        assert!(!destination.path().join(absent).exists());
    }
    Ok(())
}

#[test]
fn subprocess_bounds() {
    let mut missing = Command::new("/nonexistent/renovate");
    assert!(
        run_bounded(&mut missing, TIMEOUT, MAX_OUTPUT)
            .unwrap_err()
            .to_string()
            .contains("PATH")
    );
    for (script, timeout, limit, expected) in [
        ("printf ok", TIMEOUT, MAX_OUTPUT, "ok"),
        ("printf failure >&2; exit 7", TIMEOUT, MAX_OUTPUT, "failure"),
        (
            "exec sleep 10",
            Duration::from_millis(30),
            MAX_OUTPUT,
            "timed out",
        ),
        ("printf oversized", TIMEOUT, 2, "exceeded"),
    ] {
        let result = run_bounded(Command::new("sh").args(["-c", script]), timeout, limit);
        if script == "printf ok" {
            assert_eq!(result.unwrap(), expected);
        } else {
            assert!(result.unwrap_err().to_string().contains(expected));
        }
    }
}
