use anyhow::{Context, Result, ensure};
use rustix::fs::{CWD, Dir, Mode, OFlags, ResolveFlags, openat2};
use std::{
    env, fs,
    io::Write,
    os::{
        fd::AsFd,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, PermissionsExt, chown},
        },
    },
    path::{Component, Path, PathBuf},
    process::Command,
};

const SHARE_DIR: &str = "/etc/agent-share";

fn validate(name: &str, path: &str, content: &str) -> Result<()> {
    ensure!(
        name.bytes()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)),
        "invalid share name"
    );
    ensure!(
        path.is_empty() || content.is_empty(),
        "set path or content, not both"
    );
    Ok(())
}

fn mode(path: &Path, value: u32) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(value)).context("set share permissions")
}

fn file(path: &Path, content: &[u8]) -> Result<()> {
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .context("create shared file")?;
    output.write_all(content).context("write shared file")?;
    mode(path, 0o644)
}

// A source owner could exchange a checked path for a symlink while root
// copies it. Resolve every component without symlinks and keep the opened
// inode: neither file reads nor directory traversal resolve the path again.
fn open_source(parent: impl AsFd, path: &Path) -> Result<fs::File> {
    let fd = openat2(
        parent,
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )
    .context("open share source without symlinks (requires Linux 5.6+)")?;
    Ok(fd.into())
}

fn copy_tree(mut src: fs::File, dest: &Path) -> Result<()> {
    let metadata = src.metadata().context("inspect opened share source")?;
    if metadata.is_dir() {
        fs::create_dir(dest).context("create shared directory")?;
        mode(dest, 0o755)?;
        let mut entries = Dir::read_from(&src)
            .context("read share source directory")?
            .map(|entry| entry.map(|entry| entry.file_name().to_owned()))
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort();
        for entry in entries {
            if entry.to_bytes() == b"." || entry.to_bytes() == b".." {
                continue;
            }
            let name = Path::new(std::ffi::OsStr::from_bytes(entry.to_bytes()));
            copy_tree(open_source(&src, name)?, &dest.join(name))?;
        }
    } else {
        ensure!(
            metadata.is_file(),
            "source is neither a regular file nor a directory; refusing to share it"
        );
        // Do not copy source ownership: every destination inode is newly created.
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dest)
            .context("create shared file")?;
        std::io::copy(&mut src, &mut output).context("copy opened share source")?;
        mode(dest, 0o644)?;
    }
    Ok(())
}

fn source(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        env::current_dir()
            .context("get working directory")?
            .canonicalize()?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    Ok(normalized)
}

fn remove(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path)?,
        Ok(_) => fs::remove_file(path)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

fn root_owned(path: &Path) -> Result<()> {
    chown(path, Some(0), Some(0)).context("set root ownership")?;
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            root_owned(&entry?.path())?;
        }
    }
    Ok(())
}

fn publish(directory: &Path, name: &str, path: &str, content: &str, root: bool) -> Result<()> {
    validate(name, path, content)?;
    fs::create_dir_all(directory).context("create share root")?;
    mode(directory, 0o755)?;
    // Stage on the same filesystem; no reader sees a partially copied tree.
    let staging = tempfile::Builder::new()
        .prefix(".share-")
        .tempdir_in(directory)
        .context("create share staging directory")?;
    let tmp = staging.path().join("content");
    if path.is_empty() {
        file(&tmp, content.as_bytes())?;
    } else {
        copy_tree(open_source(CWD, &source(Path::new(path))?)?, &tmp)?;
    }
    if root {
        root_owned(&tmp)?;
    }
    let dest = directory.join(name);
    remove(&dest).context("remove previous share")?;
    fs::rename(tmp, &dest).context("publish share")?;
    Ok(())
}

pub fn run() -> Result<()> {
    ensure!(rustix::process::getuid().is_root(), "must run as root");
    let name = env::var("SHARE_NAME").unwrap_or_default();
    let path = env::var("SHARE_PATH").unwrap_or_default();
    let content = env::var("SHARE_CONTENT").unwrap_or_default();
    publish(Path::new(SHARE_DIR), &name, &path, &content, true)?;
    println!(
        "Shared {} as {SHARE_DIR}/{name} (root-owned, read-only to other users)",
        if path.is_empty() { "content" } else { &path }
    );
    Ok(())
}

pub fn check() -> Result<()> {
    let directory = Path::new(SHARE_DIR);
    for (name, value) in [
        (
            "greeting",
            format!("hello from {}", super::required("GITHUB_JOB")?),
        ),
        ("file", "two\n".into()),
        ("dir/a", "one\n".into()),
    ] {
        ensure!(
            fs::read_to_string(directory.join(name))? == value,
            "unexpected share content: {name}"
        );
    }
    ensure!(
        super::required("SYMLINK")? == "failure",
        "symlink source was not refused"
    );
    ensure!(
        fs::symlink_metadata(directory.join("shadow"))
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        "the symlink was shared"
    );
    for (name, mode) in [
        (".", 0o755),
        ("greeting", 0o644),
        ("file", 0o644),
        ("dir", 0o755),
        ("dir/a", 0o644),
    ] {
        let metadata = fs::symlink_metadata(directory.join(name))?;
        ensure!(
            metadata.uid() == 0 && metadata.gid() == 0 && metadata.mode() & 0o7777 == mode,
            "unexpected ownership/permissions: {name}"
        );
    }
    ensure!(
        fs::OpenOptions::new()
            .append(true)
            .open(directory.join("greeting"))
            .is_err(),
        "job user can write greeting"
    );
    ensure!(
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join("dir/new"))
            .is_err(),
        "job user can write shared directory"
    );
    ensure!(
        Command::new("sudo")
            .args(["-u", "nobody", "cat"])
            .arg(directory.join("greeting"))
            .status()
            .context("read share as nobody")?
            .success(),
        "nobody cannot read greeting"
    );
    println!("sandbox-share: shares are root-owned and read-only");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn replaced_source_names_cannot_redirect_open_descriptors() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let original = temp.path().join("original");
        let private = temp.path().join("private");
        fs::create_dir(&original)?;
        fs::create_dir(&private)?;
        fs::write(original.join("file"), "public")?;
        fs::write(private.join("file"), "not for publication")?;
        let directory = open_source(CWD, &original)?;
        let file = open_source(&directory, Path::new("file"))?;
        fs::rename(&original, temp.path().join("moved"))?;
        symlink(&private, &original)?;
        copy_tree(directory, &temp.path().join("copied"))?;
        assert_eq!(
            fs::read_to_string(temp.path().join("copied/file"))?,
            "public"
        );
        let moved = temp.path().join("moved/file");
        fs::remove_file(&moved)?;
        symlink(private.join("file"), &moved)?;
        copy_tree(file, &temp.path().join("copied-file"))?;
        assert_eq!(
            fs::read_to_string(temp.path().join("copied-file"))?,
            "public"
        );
        assert!(open_source(CWD, &original.join("file")).is_err());
        assert!(open_source(CWD, &moved).is_err());
        Ok(())
    }

    #[test]
    fn inputs() {
        for name in ["", ".", "..", "../escape", "a/b", "-a", "é", "a\n"] {
            assert!(validate(name, "", "").is_err(), "{name:?}");
        }
        for name in ["a", "A0._-", "0"] {
            assert!(validate(name, "", "").is_ok());
        }
        assert!(validate("a", "file", "text").is_err());
    }

    #[test]
    fn content_tree_replacement_and_symlinks() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let shares = temp.path().join("shares");
        let src = temp.path().join("src");
        fs::create_dir(&src)?;
        fs::write(src.join("a"), "one\n")?;
        mode(&src.join("a"), 0o700)?;
        for content in ["hello", "", "${{ inputs.untrusted }}\n"] {
            publish(&shares, "greeting", "", content, false)?;
            assert_eq!(fs::read_to_string(shares.join("greeting"))?, content);
            assert_eq!(
                fs::metadata(shares.join("greeting"))?.mode() & 0o7777,
                0o644
            );
        }
        publish(&shares, "tree", src.to_str().unwrap(), "", false)?;
        assert_eq!(fs::read_to_string(shares.join("tree/a"))?, "one\n");
        assert_eq!(fs::metadata(shares.join("tree"))?.mode() & 0o7777, 0o755);
        assert_eq!(fs::metadata(shares.join("tree/a"))?.mode() & 0o7777, 0o644);
        publish(&shares, "tree", "", "replacement", false)?;
        assert_eq!(fs::read_to_string(shares.join("tree"))?, "replacement");
        symlink(src.join("a"), src.join("link"))?;
        symlink(&src, temp.path().join("alias"))?;
        for path in [
            src.clone(),
            src.join("link"),
            temp.path().join("alias/a"),
            PathBuf::from("/dev/null"),
        ] {
            assert!(publish(&shares, "tree", path.to_str().unwrap(), "", false).is_err());
            assert_eq!(fs::read_to_string(shares.join("tree"))?, "replacement");
        }
        assert!(fs::read_dir(&shares)?.all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".share-")
        }));
        Ok(())
    }
}
