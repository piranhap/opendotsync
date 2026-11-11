//! Exercise the CLI in isolated subprocess environments on both macOS and Linux.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

use opendotsync::backend::{local::LocalBackend, BackendManifest, SnapshotType, StorageBackend};
use opendotsync::{config, encryption, metadata, retention, snapshot};

struct Machine {
    root: PathBuf,
    home: PathBuf,
    config: PathBuf,
    metadata: PathBuf,
}

impl Machine {
    fn init(root: &Path, store: &Path) -> Self {
        let mut machine = Self {
            root: root.to_owned(),
            home: root.join("home"),
            config: PathBuf::new(),
            metadata: PathBuf::new(),
        };
        std::fs::create_dir_all(&machine.home).unwrap();
        std::fs::write(machine.home.join(".bashrc"), "bash base\n").unwrap();
        std::fs::write(machine.home.join(".vimrc"), "vim base\n").unwrap();
        let output = machine.run(&["init", "--path", store.to_str().unwrap()]);
        machine.config = PathBuf::from(
            output
                .lines()
                .find_map(|s| s.strip_prefix("Created config: "))
                .unwrap(),
        );
        machine.metadata = PathBuf::from(
            output
                .lines()
                .find_map(|s| s.strip_prefix("Created metadata: "))
                .unwrap(),
        );
        assert!(machine.config.starts_with(root), "config must be isolated");
        assert!(
            machine.metadata.starts_with(root),
            "metadata must be isolated"
        );
        machine
    }

    fn output(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_opendotsync"))
            .env("HOME", &self.root)
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .args(["--home", self.home.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap()
    }

    fn run(&self, args: &[&str]) -> String {
        let output = self.output(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn write(&self, name: &str, content: &str) {
        std::fs::write(self.home.join(name), content).unwrap();
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.home.join(name)).unwrap()
    }
}

fn manifest(store: &Path) -> BackendManifest {
    serde_json::from_slice(&std::fs::read(store.join("default/manifest.json")).unwrap()).unwrap()
}

#[test]
fn deltas_restore_pull_diff_and_profile_merge() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let a = Machine::init(&tmp.path().join("a"), &store);
    let b = Machine::init(&tmp.path().join("b"), &store);
    a.run(&["pull"]); // v2 is the latest full base.
    a.write(".bashrc", "bash changed\n");
    a.run(&["push"]);
    let delta = std::fs::read(store.join("default/v003.tar.zst")).unwrap();
    assert_eq!(
        snapshot::list_snapshot_contents(&delta).unwrap(),
        [".bashrc"]
    );
    let meta = manifest(&store);
    assert_eq!(meta.snapshots[2].snapshot_type, SnapshotType::Delta);
    assert_eq!(meta.snapshots[2].base_version, Some(2));

    a.write(".vimrc", "vim changed\n");
    a.run(&["push"]); // v4 contains only vim, requiring the v3 delta as well.
    b.run(&["pull"]);
    assert_eq!(b.read(".bashrc"), "bash changed\n");
    assert_eq!(b.read(".vimrc"), "vim changed\n");
    assert!(b.run(&["push"]).contains("No changes"));
    assert_eq!(b.run(&["diff", "--remote"]).trim(), "No changes.");

    b.write(".bashrc", "temporary\n");
    b.run(&["restore", "3"]);
    assert_eq!(b.read(".bashrc"), "bash changed\n");
    assert_eq!(b.read(".vimrc"), "vim base\n");
    b.run(&["pull", "--overwrite"]);
    assert_eq!(b.read(".vimrc"), "vim changed\n");
    b.run(&["profiles", "merge", "default", "copy"]);
    let (_, copied) = LocalBackend::new(store.clone()).pull("copy", None).unwrap();
    assert_eq!(
        snapshot::read_file_from_snapshot(&copied, ".vimrc")
            .unwrap()
            .unwrap(),
        b"vim changed\n"
    );

    // Deletion-only pushes contain no file entries and must not resurrect the base file.
    std::fs::remove_file(a.home.join(".vimrc")).unwrap();
    a.run(&["push"]);
    let meta = manifest(&store);
    assert_eq!(meta.snapshots.last().unwrap().deleted_files, [".vimrc"]);
    let raw = std::fs::read(store.join("default/v005.tar.zst")).unwrap();
    assert!(snapshot::list_snapshot_contents(&raw).unwrap().is_empty());
    b.run(&["pull"]);
    assert!(!b.home.join(".vimrc").exists());
    assert!(b.run(&["push"]).contains("No changes"));

    a.write(".vimrc", "recreated\n");
    a.run(&["push"]);
    b.run(&["pull"]);
    assert_eq!(b.read(".vimrc"), "recreated\n");
    // A local edit survives a remote deletion and is published as a new file.
    b.write(".vimrc", "local survives deletion\n");
    std::fs::remove_file(a.home.join(".vimrc")).unwrap();
    a.run(&["push"]);
    b.run(&["pull"]);
    assert_eq!(b.read(".vimrc"), "local survives deletion\n");
    b.run(&["push"]);
    a.run(&["pull"]);
    assert_eq!(a.read(".vimrc"), "local survives deletion\n");

    // Losing the local ancestor cache falls back to a self-contained archive.
    std::fs::remove_file(a.metadata.parent().unwrap().join("parent.tar.zst")).unwrap();
    a.write(".bashrc", "without parent\n");
    a.run(&["push"]);
    assert_eq!(
        manifest(&store).snapshots.last().unwrap().snapshot_type,
        SnapshotType::Full
    );
    a.run(&["push", "--force"]);
    assert_eq!(
        manifest(&store).snapshots.last().unwrap().snapshot_type,
        SnapshotType::Full
    );
}

#[test]
fn checkpoints_retention_and_legacy_manifests() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let a = Machine::init(&tmp.path().join("a"), &store);
    // A manifest from an older release has no type, base, or deletion fields.
    let path = store.join("default/manifest.json");
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    legacy["snapshots"][0]
        .as_object_mut()
        .unwrap()
        .remove("type");
    std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert_eq!(
        manifest(&store).snapshots[0].snapshot_type,
        SnapshotType::Full
    );
    assert_eq!(
        config::load_config(Some(&a.config))
            .unwrap()
            .full_snapshot_interval
            .get(),
        10
    );
    for version in 2..=21 {
        a.write(".bashrc", &format!("version {version}\n"));
        a.run(&["push"]);
    }
    let meta = manifest(&store);
    assert_eq!(meta.snapshots.first().unwrap().version, 11);
    assert_eq!(meta.snapshots.last().unwrap().version, 21);
    assert_eq!(
        meta.snapshots.last().unwrap().snapshot_type,
        SnapshotType::Full
    );
    // Keep just v22: its full base (21) must survive too.
    a.write(".bashrc", "version 22\n");
    a.run(&["push"]);
    let backend = LocalBackend::new(store.clone());
    retention::prune(&backend, "default", 1).unwrap();
    assert_eq!(
        manifest(&store)
            .snapshots
            .iter()
            .map(|s| s.version)
            .collect::<Vec<_>>(),
        [21, 22]
    );
    a.write(".vimrc", "local damage\n");
    a.run(&["restore", "22"]);
    assert_eq!(a.read(".vimrc"), "vim base\n");
    assert_eq!(a.read(".bashrc"), "version 22\n");
    // Broken chains fail explicitly rather than restoring an incomplete archive.
    std::fs::remove_file(store.join("default/v021.tar.zst")).unwrap();
    assert!(!a.output(&["restore", "22"]).status.success());
}

#[test]
fn encrypted_deltas_and_configurable_interval() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let a = Machine::init(&tmp.path().join("a"), &store);
    let key = tmp.path().join("identity.age");
    encryption::generate_keypair(&key).unwrap();
    let mut cfg = config::load_config(Some(&a.config)).unwrap();
    cfg.encryption.enabled = true;
    cfg.encryption.identity = Some(key);
    cfg.full_snapshot_interval = std::num::NonZeroU64::new(3).unwrap();
    config::save_config(&a.config, &cfg).unwrap();
    // The existing base and parent need encrypting when enabling encryption.
    for path in [
        store.join("default/v001.tar.zst"),
        a.metadata.parent().unwrap().join("parent.tar.zst"),
    ] {
        let plain = std::fs::read(&path).unwrap();
        std::fs::write(path, encryption::encrypt(&plain, &cfg.encryption).unwrap()).unwrap();
    }
    for version in 2..=4 {
        a.write(".bashrc", &format!("encrypted {version}\n"));
        a.run(&["push"]);
    }
    let meta = manifest(&store);
    assert_eq!(meta.snapshots[1].snapshot_type, SnapshotType::Delta);
    assert_eq!(meta.snapshots[2].snapshot_type, SnapshotType::Delta);
    assert_eq!(meta.snapshots[3].snapshot_type, SnapshotType::Full);
    let delta = std::fs::read(store.join("default/v002.tar.zst")).unwrap();
    assert!(snapshot::extract_snapshot(&delta).is_err());
    let decoded = encryption::decrypt(&delta, &cfg.encryption).unwrap();
    assert_eq!(
        snapshot::list_snapshot_contents(&decoded).unwrap(),
        [".bashrc"]
    );
    a.run(&["restore", "3"]);
    assert_eq!(a.read(".bashrc"), "encrypted 3\n");
    assert_eq!(a.read(".vimrc"), "vim base\n");
    a.run(&["pull", "--overwrite"]);
    assert_eq!(a.read(".bashrc"), "encrypted 4\n");
    assert!(a.run(&["push"]).contains("No changes"));
    let parent = std::fs::read(a.metadata.parent().unwrap().join("parent.tar.zst")).unwrap();
    assert!(snapshot::extract_snapshot(&parent).is_err());
    let state = metadata::load_metadata_from(&a.metadata).unwrap().unwrap();
    assert_eq!(state.version, 4);
}
