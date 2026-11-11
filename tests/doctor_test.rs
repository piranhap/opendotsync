#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

fn run(root: &Path, config: &str, binary: Option<(&str, &str)>) -> Output {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    if let Some((name, script)) = binary {
        let path = bin.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let config_path = root.join("custom.yaml");
    std::fs::write(&config_path, config).unwrap();
    Command::new(env!("CARGO_BIN_EXE_opendotsync"))
        .args(["--verbose", "--config"])
        .arg(config_path)
        .arg("--home")
        .arg(root)
        .arg("doctor")
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("PATH", bin)
        .env("OPENDOTSYNC_GITHUB_TOKEN", "VERY_SECRET_TOKEN")
        .output()
        .unwrap()
}

#[test]
fn missing_git_is_actionable_and_other_checks_continue() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(
        tmp.path(),
        "backend:\n  type: github\n  repo: https://github.com/u/r.git\n",
        None,
    );
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("git unavailable"), "{text}");
    assert!(text.contains("install git and check PATH"));
    assert!(text.contains("Checking encryption"));
    assert!(text.contains("Remote version: unknown"));
}

#[test]
fn git_auth_failure_is_redacted_even_in_verbose_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(tmp.path(), "backend:\n  type: github\n  repo: https://github.com/u/r.git\n", Some(("git", "#!/bin/sh\ncase \"$*\" in\n  *--version*) echo 'git version test'; exit 0;;\nesac\necho \"permission denied https://x-access-token:${OPENDOTSYNC_GITHUB_TOKEN}@github.com/u/r.git\" >&2\nexit 1\n")));
    assert!(!out.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("permission denied"), "{text}");
    assert!(!text.contains("VERY_SECRET_TOKEN"), "{text}");
    assert!(text.contains("Remote version: unknown"));
}

#[test]
fn rclone_checks_configured_remote_and_preserves_manifest_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(tmp.path(), "backend:\n  type: rclone\n  remote: myremote\n  path: dotfiles\n", Some(("rclone", "#!/bin/sh\ncase \"$1\" in\n --version) echo 'rclone test'; exit 0;;\n lsd) [ \"$2\" = myremote:dotfiles ] || exit 9; exit 0;;\n cat) echo 'permission denied reading manifest' >&2; exit 1;;\nesac\nexit 9\n")));
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("✓ Remote reachable"), "{text}");
    assert!(text.contains("permission denied reading manifest"));
    assert!(text.contains("Remote version: unknown"));
    assert!(text.contains("Write access not tested"));
}
