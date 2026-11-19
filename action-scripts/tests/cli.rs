use std::{fs, os::unix::fs::symlink, process::Command};

const BINARY: &str = env!("CARGO_BIN_EXE_action-scripts");

#[test]
fn setup_appends_only_validated_outputs() {
    let temp = tempfile::tempdir().unwrap();
    let settings = temp.path().join("settings.json");
    let output = temp.path().join("output");
    fs::write(
        &settings,
        r#"{"known":{"oauth-client-id":"client","tags":"tag:test","proxy":"http://192.0.2.1"}}"#,
    )
    .unwrap();
    for (owner, client, tags, proxy, success) in [
        ("known", "", "", "", true),
        ("unknown", "", "", "", false),
        ("unknown", "custom", "tag:custom", "", true),
        ("unknown", "custom", "", "", false),
        ("known", "", "", "value\ninjected=true", false),
        ("known", "", "", "value\rinjected=true", false),
    ] {
        fs::write(&output, "existing=value\n").unwrap();
        let result = Command::new(BINARY)
            .arg("tailscale-setup")
            .arg(&settings)
            .env("REPOSITORY_OWNER", owner)
            .env("OAUTH_CLIENT_ID", client)
            .env("TAGS", tags)
            .env("PROXY", proxy)
            .env("AUDIENCE", "")
            .env("GITHUB_OUTPUT", &output)
            .output()
            .unwrap();
        assert_eq!(result.status.success(), success, "{result:?}");
        let output = fs::read_to_string(&output).unwrap();
        if success {
            let (client, tags, proxy) = if owner == "known" {
                ("client", "tag:test", "http://192.0.2.1")
            } else {
                (client, tags, proxy)
            };
            assert_eq!(
                output,
                format!(
                    "existing=value\noauth-client-id={client}\ntags={tags}\nproxy={proxy}\naudience=api.tailscale.com/{client}\n"
                )
            );
        } else {
            assert_eq!(output, "existing=value\n");
        }
    }
}

// This mutates /etc/agent-share. Run only on a disposable CI runner with
// passwordless sudo; the ordinary unit tests need no privilege or network.
#[test]
#[ignore = "requires disposable runner and passwordless sudo"]
fn root_share_dry_run() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("dir")).unwrap();
    fs::write(temp.path().join("dir/a"), "one\n").unwrap();
    fs::write(temp.path().join("file"), "two\n").unwrap();
    symlink("/etc/shadow", temp.path().join("shadow")).unwrap();
    for (name, source, content, success) in [
        ("greeting", "", "hello from rust-dry-run", true),
        ("file", "file", "", true),
        ("dir", "dir", "", true),
        ("shadow", "shadow", "", false),
    ] {
        let path = if source.is_empty() {
            String::new()
        } else {
            temp.path().join(source).to_str().unwrap().to_owned()
        };
        let result = Command::new("sudo")
            .args(["-n", "env"])
            .arg(format!("SHARE_NAME={name}"))
            .arg(format!("SHARE_PATH={path}"))
            .arg(format!("SHARE_CONTENT={content}"))
            .args([BINARY, "sandbox-share"])
            .output()
            .unwrap();
        assert_eq!(result.status.success(), success, "{result:?}");
    }
    let result = Command::new(BINARY)
        .arg("sandbox-share-check")
        .env("GITHUB_JOB", "rust-dry-run")
        .env("SYMLINK", "failure")
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
}
