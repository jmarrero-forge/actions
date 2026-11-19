//! Execute the action's scope guard without minting credentials. Node is only
//! needed to read the still-JavaScript remint helper's exported permission list.

use std::path::{Path, PathBuf};
use std::process::Command;
use yaml_rust2::{Yaml, YamlLoader};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}

fn action() -> Yaml {
    let text = std::fs::read_to_string(root().join("app-token/action.yml")).unwrap();
    YamlLoader::load_from_str(&text).unwrap().remove(0)
}

#[test]
fn scope_is_checked_before_a_pinned_mint_and_permissions_match_remint() {
    let action = action();
    let steps = action["runs"]["steps"].as_vec().unwrap();
    let guard = &steps[0];
    assert_eq!(guard["name"].as_str(), Some("Check the scope"));
    assert_eq!(guard["shell"].as_str(), Some("bash"));
    assert!(guard["if"].is_badvalue());
    assert!(guard["continue-on-error"].is_badvalue());
    assert_eq!(guard["env"]["LC_ALL"].as_str(), Some("C"));
    assert_eq!(
        guard["env"]["REPOSITORIES"].as_str(),
        Some("${{ inputs.repositories }}")
    );
    let uses: Vec<_> = steps
        .iter()
        .filter_map(|step| step["uses"].as_str())
        .collect();
    assert_eq!(uses.len(), 1);
    let sha = uses[0]
        .strip_prefix("actions/create-github-app-token@")
        .unwrap();
    assert_eq!(sha.len(), 40);
    assert!(
        sha.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    let mint = steps
        .iter()
        .find(|step| step["uses"].as_str().is_some())
        .unwrap();
    assert_eq!(
        mint["with"]["repositories"].as_str(),
        Some("${{ inputs.repositories }}")
    );
    let inputs: Vec<_> = action["inputs"]
        .as_hash()
        .unwrap()
        .keys()
        .filter_map(|key| key.as_str().unwrap().strip_prefix("permission-"))
        .collect();
    let output = Command::new("node").current_dir(root())
        .args(["--input-type=module", "-e", "import { PERMISSIONS } from './app-token/remint.mjs'; console.log(JSON.stringify(PERMISSIONS));"])
        .output().expect("run Node to read remint's permission contract");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let permissions: Vec<String> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        inputs
            .iter()
            .map(|name| name.replace('-', "_"))
            .collect::<Vec<_>>(),
        permissions
    );
    let expressions: Vec<_> = inputs
        .iter()
        .map(|name| format!("${{{{ inputs.permission-{name} }}}}"))
        .collect();
    assert_eq!(
        guard["env"]["PERMISSIONS"].as_str().unwrap(),
        expressions.join(" ")
    );
    for (name, expression) in inputs.iter().zip(&expressions) {
        assert_eq!(
            mint["with"][format!("permission-{name}").as_str()].as_str(),
            Some(expression.as_str()),
            "{name}"
        );
    }
}

#[test]
fn scope_guard_rejects_empty_or_ambiguous_scope_before_minting() {
    let action = action();
    let script = action["runs"]["steps"][0]["run"].as_str().unwrap();
    let mut cases = vec![
        ("".to_owned(), "write", 1),
        (" ,\n".to_owned(), "write", 1),
        ("actions".to_owned(), "", 1),
        ("actions".to_owned(), "      ", 1),
        ("actions".to_owned(), "   write   ", 0),
        ("a,\nb".to_owned(), "read write", 0),
        ("a\r\nb.c_d-1\r\n".to_owned(), "write", 0),
    ];
    // Upstream's JavaScript trim accepts these; accepting an empty name here
    // could accidentally grant access to the whole installation.
    for space in ['\u{a0}', '\u{feff}', '\u{2028}', '\u{3000}'] {
        for value in [
            space.to_string(),
            format!("a,{space}"),
            format!(" ,{space},{space}\r"),
        ] {
            cases.push((value, "write", 1));
        }
    }
    for value in [
        "\r",
        " \r ",
        "\r,\r",
        "\r\n,\t\u{b}\u{c}",
        "owner/repo",
        "a;id",
        "*",
    ] {
        cases.push((value.to_owned(), "write", 1));
    }
    for (repositories, permissions, expected) in cases {
        let output = Command::new("bash")
            .args(["--noprofile", "--norc", "-c", script])
            .env_clear()
            .env("REPOSITORIES", &repositories)
            .env("PERMISSIONS", permissions)
            .env("LC_ALL", "C")
            .output()
            .expect("execute the actual scope guard with Bash");
        assert_eq!(
            output.status.code(),
            Some(expected),
            "repositories={repositories:?}, permissions={permissions:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
