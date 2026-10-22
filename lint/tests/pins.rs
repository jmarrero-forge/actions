//! The linters' images are pinned by digest in Containerfile.pin, and the
//! Justfile runs exactly those references and no image of its own.

use std::path::Path;
use std::process::Command;

const LINTERS: [&str; 2] = ["actionlint", "zizmor"];

fn dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// NAME:TAG@sha256:DIGEST: the tag for Dependabot and readers, the digest
/// for what is pulled.
fn digest_pinned(image: &str) -> bool {
    let Some((name_tag, digest)) = image.split_once("@sha256:") else {
        return false;
    };
    let tagged = name_tag
        .rsplit_once(':')
        .is_some_and(|(name, tag)| name.contains('/') && !tag.is_empty() && !tag.contains('/'));
    tagged
        && digest.len() == 64
        && digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn pins() -> Vec<String> {
    std::fs::read_to_string(dir().join("Containerfile.pin"))
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.strip_prefix("FROM ").expect(line).to_owned())
        .collect()
}

#[test]
fn pinned_by_digest() {
    let pins = pins();
    assert_eq!(pins.len(), LINTERS.len(), "{pins:?}");
    for image in &pins {
        assert!(digest_pinned(image), "not NAME:TAG@sha256:DIGEST: {image}");
    }
    assert!(!digest_pinned("docker.io/rhysd/actionlint:1.7.12"));
    assert!(!digest_pinned(&format!("docker.io/rhysd/actionlint@sha256:{:064}", 0)));
}

#[test]
fn justfile_runs_the_pins() {
    let justfile = std::fs::read_to_string(dir().join("Justfile")).unwrap();
    for registry in ["docker.io/", "ghcr.io/", "quay.io/"] {
        assert!(!justfile.contains(registry), "the Justfile names an image itself");
    }
    let pins = pins();
    for linter in LINTERS {
        let out = Command::new(std::env::var("JUST").unwrap_or("just".into()))
            .arg("--justfile")
            .arg(dir().join("Justfile"))
            .args(["--evaluate", &format!("{linter}_image")])
            .output()
            .expect("just, as JUST from the test recipe or on the path");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let image = String::from_utf8(out.stdout).unwrap();
        let want = pins.iter().find(|p| p.contains(&format!("/{linter}:")));
        assert_eq!(Some(&image), want, "{linter}");
    }
}
