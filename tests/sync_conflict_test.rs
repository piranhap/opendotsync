//! Regression test for issue #1: after a pull that conflicts (local kept), the
//! next push must still create a new version containing the local content —
//! otherwise the local change is silently dropped on other machines.
//!
//! Linux-only: it relies on `dirs` honoring XDG_{CONFIG,DATA}_HOME to redirect
//! config/state into a tempdir. (macOS `dirs` ignores XDG.)
#![cfg(target_os = "linux")]

use std::path::Path;

use opendotsync::{config, paths, sync};

/// Point config + state dirs at a per-machine root so two simulated machines
/// share one backend store but keep independent local state.
fn use_machine(root: &Path) {
    std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
    std::env::set_var("XDG_DATA_HOME", root.join("data"));
}

#[test]
fn push_after_conflict_keeps_local_change() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let store = base.join("store");
    std::fs::create_dir_all(&store).unwrap();

    let home_a = base.join("home_a");
    let home_b = base.join("home_b");
    std::fs::create_dir_all(&home_a).unwrap();
    std::fs::create_dir_all(&home_b).unwrap();

    // Machine A: .bashrc = "line1", init pushes v1.
    std::fs::write(home_a.join(".bashrc"), "line1\n").unwrap();
    use_machine(&base.join("machine_a"));
    sync::cmd_init(&home_a, "local", Some(&store), "default").unwrap();

    // Machine B: a different .bashrc, init pushes v2 to the same store.
    std::fs::write(home_b.join(".bashrc"), "from_b\n").unwrap();
    use_machine(&base.join("machine_b"));
    sync::cmd_init(&home_b, "local", Some(&store), "default").unwrap();

    // Machine A makes a LOCAL, unpushed edit, then pulls v2 → conflict (keep local).
    use_machine(&base.join("machine_a"));
    std::fs::write(home_a.join(".bashrc"), "line1\nalias gs=\"git status\"\n").unwrap();
    let cfg = config::load_config(None).unwrap();
    let pl = paths::load_paths(None).unwrap();

    let pull = sync::cmd_pull(&cfg, &pl, &home_a, None, false).unwrap();
    assert_eq!(
        pull.conflicts,
        vec![".bashrc".to_string()],
        "expected a conflict that keeps the local .bashrc"
    );

    // The bug: this push reported "No changes" and never created v3, dropping
    // A's alias. The fix stores the remote checksum for the conflicted file,
    // so push detects the divergence and creates a new version.
    let pushed = sync::cmd_push(&cfg, &pl, &home_a, false, None).unwrap();
    assert_eq!(
        pushed, 3,
        "push after a kept-local conflict must create a new version"
    );

    // Machine B pulls and must receive A's local content.
    use_machine(&base.join("machine_b"));
    let cfg_b = config::load_config(None).unwrap();
    let pl_b = paths::load_paths(None).unwrap();
    sync::cmd_pull(&cfg_b, &pl_b, &home_b, None, false).unwrap();
    let b_bashrc = std::fs::read_to_string(home_b.join(".bashrc")).unwrap();
    assert!(
        b_bashrc.contains("alias gs="),
        "machine B should have received A's alias, got: {b_bashrc:?}"
    );
}
