//! CLI regression tests for issue #5. Each child process has isolated config,
//! state, home, and storage directories, including on macOS.
#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::path::PathBuf;
use std::process::{Command, Output};

struct Machine {
    _root: tempfile::TempDir,
    home: PathBuf,
    config: PathBuf,
    state: PathBuf,
    store: PathBuf,
}

impl Machine {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        #[cfg(target_os = "macos")]
        let (config, state) = {
            let dir = home.join("Library/Application Support/opendotsync");
            (dir.clone(), dir)
        };
        #[cfg(target_os = "linux")]
        let (config, state) = (
            home.join("config/opendotsync"),
            home.join("data/opendotsync"),
        );
        let store = root.path().join("store");
        Self {
            _root: root,
            home,
            config,
            state,
            store,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_opendotsync"))
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .arg("--home")
            .arg(&self.home)
            .args(args)
            .output()
            .unwrap()
    }

    fn init(&self, args: &[&str]) -> Output {
        let mut init_args = vec![
            "init",
            "--backend",
            "local",
            "--path",
            self.store.to_str().unwrap(),
        ];
        init_args.extend_from_slice(args);
        self.run(&init_args)
    }
}

fn assert_success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn repeated_init_refuses_and_preserves_state() {
    let machine = Machine::new();
    std::fs::write(machine.home.join(".bashrc"), "initial\n").unwrap();
    assert_success(machine.init(&["--profile", "personal"]));
    for content in ["second\n", "third\n"] {
        std::fs::write(machine.home.join(".bashrc"), content).unwrap();
        assert_success(machine.run(&["push"]));
    }
    let paths_path = machine.config.join("paths.yaml");
    let mut paths = std::fs::read_to_string(&paths_path).unwrap();
    paths.push_str("\n# custom paths configuration\n");
    std::fs::write(&paths_path, paths).unwrap();
    let files = [
        machine.config.join("config.yaml"),
        paths_path,
        machine.state.join("metadata.json"),
        machine.state.join("parent.tar.zst"),
    ];
    let before: Vec<_> = files
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();

    let output = machine.init(&[]);
    assert!(
        !output.status.success(),
        "repeated init must refuse to overwrite state"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains(
        "Already initialised (profile: personal, version: 3). Use --reinit to overwrite."
    ));
    for (path, expected) in files.iter().zip(before) {
        assert_eq!(
            std::fs::read(path).unwrap(),
            expected,
            "{} changed",
            path.display()
        );
    }
}

#[test]
fn reinit_explicitly_replaces_existing_state() {
    let machine = Machine::new();
    std::fs::write(machine.home.join(".bashrc"), "initial\n").unwrap();
    assert_success(machine.init(&["--profile", "old"]));
    std::fs::write(
        machine.config.join("paths.yaml"),
        "include: []\nexclude: []\n",
    )
    .unwrap();
    assert_success(machine.init(&["--reinit", "--profile", "new"]));

    let config =
        opendotsync::config::load_config(Some(&machine.config.join("config.yaml"))).unwrap();
    assert_eq!(config.profile, "new");
    let meta = opendotsync::metadata::load_metadata_from(&machine.state.join("metadata.json"))
        .unwrap()
        .unwrap();
    assert_eq!(meta.profile, "new");
    assert_eq!(meta.version, 1);
    assert!(meta.checksums.contains_key(".bashrc"));
}

#[test]
fn partial_or_corrupt_state_still_requires_reinit() {
    for filename in ["config.yaml", "paths.yaml", "metadata.json"] {
        let machine = Machine::new();
        let dir = if filename == "metadata.json" {
            &machine.state
        } else {
            &machine.config
        };
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(filename);
        let contents = b"invalid existing state that must not be overwritten";
        std::fs::write(&path, contents).unwrap();

        let output = machine.init(&[]);
        assert!(
            !output.status.success(),
            "existing {filename} must block init"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("Use --reinit to overwrite."));
        assert_eq!(std::fs::read(path).unwrap(), contents);
        assert!(
            !machine.store.exists(),
            "refused init must not touch the backend"
        );

        assert_success(machine.init(&["--reinit"]));
    }
}

#[test]
fn config_without_metadata_reports_existing_profile() {
    let machine = Machine::new();
    assert_success(machine.init(&["--profile", "personal"]));
    std::fs::remove_file(machine.state.join("metadata.json")).unwrap();
    let output = machine.init(&[]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains(
        "Already initialised (profile: personal, version: unknown). Use --reinit to overwrite."
    ));
}

#[test]
fn reinit_is_allowed_on_a_fresh_machine() {
    let machine = Machine::new();
    assert_success(machine.init(&["--reinit"]));
}
