//! Issue #9: conflict commands accept paths users copy from their shell.
#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _root: tempfile::TempDir,
    home: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let state = if cfg!(target_os = "macos") {
            home.join("Library/Application Support/opendotsync")
        } else {
            home.join(".local/share/opendotsync")
        };
        Self {
            _root: root,
            home,
            state,
        }
    }

    fn run(&self, action: &str, path: Option<&str>) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_opendotsync"));
        cmd.env("HOME", &self.home)
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .arg("--home")
            .arg(&self.home)
            .args(["conflicts", action]);
        if let Some(path) = path {
            cmd.arg(path);
        }
        cmd.output().unwrap()
    }

    fn stash(&self, version: u64, path: &str, content: &[u8]) -> PathBuf {
        opendotsync::conflicts::stash_conflict(&self.state, version, path, content).unwrap()
    }
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn conflict_commands_accept_relative_absolute_and_tilde_paths() {
    let f = Fixture::new();
    let path = ".config/shell/rc";
    let local = f.home.join(path);
    std::fs::create_dir_all(local.parent().unwrap()).unwrap();
    for input in [
        path.to_string(),
        local.to_string_lossy().into_owned(),
        format!("~/{path}"),
    ] {
        std::fs::write(&local, b"local contents").unwrap();
        let stash = f.stash(1, path, b"remote contents");
        let shown = success(f.run("show", Some(&input)));
        assert!(shown.contains("local contents"));
        assert!(shown.contains("remote contents"));
        success(f.run("accept-local", Some(&input)));
        assert!(!stash.exists());
        assert_eq!(std::fs::read(&local).unwrap(), b"local contents");
        f.stash(1, path, b"remote contents");
        success(f.run("accept-remote", Some(&input)));
        assert_eq!(std::fs::read(&local).unwrap(), b"remote contents");
        assert!(!stash.exists());
    }
}

#[test]
fn missing_conflict_errors_name_the_path_and_suggest_listing() {
    let f = Fixture::new();
    for with_other_conflict in [false, true] {
        if with_other_conflict {
            f.stash(2, ".vimrc", b"other");
        }
        for action in ["show", "accept-local", "accept-remote"] {
            let output = f.run(action, Some(f.home.join(".bashrc").to_str().unwrap()));
            assert!(
                !output.status.success(),
                "{action} must report a missing stash"
            );
            assert!(String::from_utf8_lossy(&output.stderr).contains(
                "No conflict stashed for '.bashrc'. Run 'conflicts list' to see active conflicts."
            ), "{}", String::from_utf8_lossy(&output.stderr));
        }
    }
}

#[test]
fn empty_list_suggests_pull_even_after_resolving() {
    let f = Fixture::new();
    assert!(success(f.run("list", None)).contains("opendotsync pull"));
    f.stash(1, ".bashrc", b"remote");
    success(f.run("accept-local", Some(".bashrc")));
    let output = success(f.run("list", None));
    assert!(output.contains("opendotsync pull"));
    assert!(!output.contains("v1:"));
}

#[test]
fn commands_find_latest_stash_for_the_requested_file() {
    let f = Fixture::new();
    let old = f.stash(1, ".bashrc", b"old");
    let latest = f.stash(2, ".bashrc", b"latest");
    let other = f.stash(3, ".vimrc", b"other");
    assert!(success(f.run("show", Some(".bashrc"))).contains("latest"));
    success(f.run("accept-remote", Some(".bashrc")));
    assert_eq!(std::fs::read(f.home.join(".bashrc")).unwrap(), b"latest");
    assert!(!old.exists());
    assert!(!latest.exists());
    assert!(other.exists());
}

#[test]
fn paths_outside_home_cannot_be_used_as_stashes() {
    let f = Fixture::new();
    let outside = f._root.path().join("outside");
    std::fs::write(&outside, b"untouched").unwrap();
    f.stash(1, ".bashrc", b"remote");
    for input in [outside.as_path(), Path::new("../outside")] {
        for action in ["show", "accept-local", "accept-remote"] {
            assert!(!f.run(action, input.to_str()).status.success());
            assert_eq!(std::fs::read(&outside).unwrap(), b"untouched");
        }
    }
}
