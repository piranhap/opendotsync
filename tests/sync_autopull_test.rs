//! Regression test for issue #4: `push` from a machine behind the remote should
//! auto-pull+merge and then push, instead of failing with a version conflict —
//! and a local edit to a file the remote did not touch must be preserved and
//! reach the other machine. Also checks that --no-auto-pull still hard-errors.
//!
//! Linux-only (relies on `dirs` honoring XDG_{CONFIG,DATA}_HOME). A single test
//! function so the process-global env vars it sets are never touched concurrently.
#![cfg(target_os = "linux")]

use std::path::Path;

use opendotsync::{config, paths, sync};

fn use_machine(root: &Path) {
    std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
    std::env::set_var("XDG_DATA_HOME", root.join("data"));
}

#[test]
fn push_auto_pull_behaviour() {
    // --- Scenario 1: auto-pull on → push succeeds and preserves the local edit ---
    {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let store = base.join("store");
        std::fs::create_dir_all(&store).unwrap();
        let home_a = base.join("home_a");
        let home_b = base.join("home_b");
        std::fs::create_dir_all(&home_a).unwrap();
        std::fs::create_dir_all(&home_b).unwrap();

        // Machine A: .bashrc + .vimrc, init pushes v1.
        std::fs::write(home_a.join(".bashrc"), "bash-base\n").unwrap();
        std::fs::write(home_a.join(".vimrc"), "vim-base\n").unwrap();
        use_machine(&base.join("machine_a"));
        sync::cmd_init(&home_a, "local", Some(&store), "default").unwrap();

        // Machine B: same content, init pushes v2 so the store advances past A.
        std::fs::write(home_b.join(".bashrc"), "bash-base\n").unwrap();
        std::fs::write(home_b.join(".vimrc"), "vim-base\n").unwrap();
        use_machine(&base.join("machine_b"));
        sync::cmd_init(&home_b, "local", Some(&store), "default").unwrap();

        // Machine A behind (local v1, remote v2). It edits .vimrc — a file the
        // remote did NOT change — and pushes WITHOUT pulling first.
        use_machine(&base.join("machine_a"));
        std::fs::write(home_a.join(".vimrc"), "vim-base\nset number\n").unwrap();
        let cfg = config::load_config(None).unwrap();
        let pl = paths::load_paths(None).unwrap();
        let version = sync::cmd_push(&cfg, &pl, &home_a, false, None, false).unwrap();
        assert_eq!(version, 3, "push should auto-pull then create v3");

        // Machine B pulls v3 and must see A's .vimrc edit (proves it wasn't stranded).
        use_machine(&base.join("machine_b"));
        let cfg_b = config::load_config(None).unwrap();
        let pl_b = paths::load_paths(None).unwrap();
        sync::cmd_pull(&cfg_b, &pl_b, &home_b, None, false).unwrap();
        let b_vimrc = std::fs::read_to_string(home_b.join(".vimrc")).unwrap();
        assert!(
            b_vimrc.contains("set number"),
            "machine B should have received A's .vimrc edit, got: {b_vimrc:?}"
        );
    }

    // --- Scenario 2: --no-auto-pull while behind still hard-errors ---
    {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let store = base.join("store");
        std::fs::create_dir_all(&store).unwrap();
        let home_a = base.join("home_a");
        let home_b = base.join("home_b");
        std::fs::create_dir_all(&home_a).unwrap();
        std::fs::create_dir_all(&home_b).unwrap();

        std::fs::write(home_a.join(".bashrc"), "base\n").unwrap();
        use_machine(&base.join("machine_a"));
        sync::cmd_init(&home_a, "local", Some(&store), "default").unwrap();

        std::fs::write(home_b.join(".bashrc"), "base\n").unwrap();
        use_machine(&base.join("machine_b"));
        sync::cmd_init(&home_b, "local", Some(&store), "default").unwrap();

        use_machine(&base.join("machine_a"));
        std::fs::write(home_a.join(".bashrc"), "base\nlocal-edit\n").unwrap();
        let cfg = config::load_config(None).unwrap();
        let pl = paths::load_paths(None).unwrap();
        let err = sync::cmd_push(&cfg, &pl, &home_a, false, None, true);
        assert!(err.is_err(), "no-auto-pull push while behind must error");
    }
}
