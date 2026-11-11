use super::*;
use std::fmt::Write;
use std::process::Command;

#[derive(Default)]
pub struct DoctorReport {
    pub has_errors: bool,
    text: String,
}

impl fmt::Display for DoctorReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl DoctorReport {
    fn section(&mut self, title: &str) {
        let _ = writeln!(self.text, "\n{title}");
    }

    fn line(&mut self, marker: &str, message: impl fmt::Display) {
        self.has_errors |= marker == "✗";
        let _ = writeln!(self.text, "  {marker} {message}");
    }

    fn check<T, E: fmt::Display>(&mut self, result: Result<T, E>, label: &str) -> Option<T> {
        match result {
            Ok(value) => {
                self.line("✓", label);
                Some(value)
            }
            Err(error) => {
                self.line("✗", format!("{label}: {error}"));
                None
            }
        }
    }
}

fn check_binary(report: &mut DoctorReport, name: &str) -> bool {
    let result = Command::new(name)
        .arg("--version")
        .output()
        .and_then(|out| {
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string())
            } else {
                Err(std::io::Error::other("version command failed"))
            }
        });
    match result {
        Ok(version) => {
            let path = std::env::var_os("PATH").and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|p| p.join(name))
                    .find(|p| p.is_file())
            });
            report.line(
                "✓",
                format!(
                    "{name} binary found: {} ({version})",
                    path.unwrap_or_else(|| name.into()).display()
                ),
            );
            true
        }
        Err(error) => {
            report.line(
                "✗",
                format!("{name} unavailable: {error} — install {name} and check PATH"),
            );
            false
        }
    }
}

/// Diagnose independently so malformed or missing configuration does not hide
/// problems in paths or state. No sync, init, or persistent cache updates occur.
pub fn cmd_doctor(config_path: Option<&Path>, home_dir: &Path) -> DoctorReport {
    diagnose(
        config_path.unwrap_or(&crate::config::default_config_path()),
        &paths::default_paths_path(),
        &metadata::state_dir(),
        home_dir,
    )
}

fn diagnose(config_path: &Path, paths_path: &Path, state_dir: &Path, home: &Path) -> DoctorReport {
    let mut report = DoctorReport::default();
    report.section("Checking configuration...");
    let config = report.check(
        crate::config::load_config(Some(config_path)),
        &format!(
            "Config file: {} (run `opendotsync init` if missing)",
            config_path.display()
        ),
    );
    let paths = report.check(
        paths::load_paths(Some(paths_path)),
        &format!("Paths file: {}", paths_path.display()),
    );
    if state_dir.is_dir() {
        report.line("✓", format!("State dir exists: {}", state_dir.display()));
    } else {
        report.line(
            "✗",
            format!(
                "State dir missing: {} — run `opendotsync init`",
                state_dir.display()
            ),
        );
    }
    let local = match metadata::load_metadata_from(&state_dir.join("metadata.json")) {
        Ok(Some(meta)) => {
            report.line("✓", "Local metadata readable");
            Some(meta)
        }
        Ok(None) => {
            report.line("✗", "Local metadata missing — run `opendotsync init`");
            None
        }
        Err(error) => {
            report.line("✗", format!("Local metadata: {error}"));
            None
        }
    };

    let mut remote_version = None;
    if let Some(config) = &config {
        let (name, prerequisites) = match &config.backend {
            BackendConfig::Local { .. } => ("local", true),
            BackendConfig::Rclone { .. } => {
                report.section("Checking backend (rclone)...");
                ("rclone", check_binary(&mut report, "rclone"))
            }
            BackendConfig::GitHub { repo, branch } => {
                report.section("Checking backend (github)...");
                let binary = check_binary(&mut report, "git");
                // Validate the original URL before the constructor strips credentials.
                let url = report
                    .check(
                        GitHubBackend::validate_repo_url(repo),
                        &format!("Repo URL valid: {}", GitHubBackend::strip_credentials(repo)),
                    )
                    .is_some();
                let branch_ok = !branch.is_empty() && !branch.starts_with('-');
                if !branch_ok {
                    report.line("✗", "Branch must be non-empty and cannot start with '-'");
                }
                let cache = GitHubBackend::cache_dir_for_repo(repo);
                if cache.is_dir() {
                    report.line("✓", format!("Cache dir exists: {}", cache.display()));
                } else {
                    report.line(
                        "⚠",
                        format!(
                            "Cache dir missing: {} — created on first sync",
                            cache.display()
                        ),
                    );
                }
                ("github", binary && url && branch_ok)
            }
            BackendConfig::S3 { .. } => ("s3", true),
            BackendConfig::Sftp { .. } => ("sftp", true),
            BackendConfig::Git { .. } => ("git", true),
        };
        if !matches!(name, "github" | "rclone") {
            report.section(&format!("Checking backend ({name})..."));
        }
        if prerequisites {
            if let Some(backend) = report.check(create_backend(config), "Backend configured") {
                if report
                    .check(
                        backend.check_connectivity(),
                        "Remote reachable (check credentials or SSH keys on failure)",
                    )
                    .is_some()
                {
                    if let BackendConfig::GitHub { repo, branch } = &config.backend {
                        report.check(GitHubBackend::new(repo.clone(), branch.clone()).check_push_access(), "Push access (dry run; server hooks and branch protection are not verified)");
                    }
                    if name == "rclone" {
                        report.line(
                            "⚠",
                            "Write access not tested (listing verifies read access only)",
                        );
                    }
                    remote_version = report.check(
                        backend.diagnostic_version(&config.profile),
                        "Remote version readable",
                    );
                }
            }
        }
    } else {
        report.section("Checking backend...");
        report.line("⚠", "Skipped — fix configuration first");
    }

    report.section("Checking tracked paths...");
    if let Some(paths) = &paths {
        for pattern in &paths.exclude {
            report.check(
                glob::Pattern::new(pattern),
                &format!("Exclude pattern: {pattern}"),
            );
        }
        for entry in &paths.include {
            if !home.join(&entry.path).exists() {
                report.line("⚠", format!("{} listed but file not found", entry.path));
            }
        }
        match collector::collect_files(home, paths) {
            Ok(files) => {
                report.line(
                    if files.is_empty() { "⚠" } else { "✓" },
                    format!(
                        "{} files matched ({} projected)",
                        files.len(),
                        format_size(collector::total_size(&files))
                    ),
                );
                for warning in collector::check_size_limits(&files) {
                    report.line("⚠", warning.message);
                }
            }
            Err(error) => report.line("✗", error),
        }
    } else {
        report.line("⚠", "Skipped — fix paths file first");
    }

    report.section("Checking encryption...");
    if let Some(config) = &config {
        if config.encryption.enabled {
            // Encrypt/decrypt validates the secret key itself, not a public-key comment.
            let result = encryption::encrypt(b"doctor", &config.encryption)
                .and_then(|data| encryption::decrypt(&data, &config.encryption));
            report.check(
                result,
                "Encryption identity usable (check encryption.identity on failure)",
            );
        } else {
            report.line("✓", "Encryption disabled");
        }
    } else {
        report.line("⚠", "Skipped — fix configuration first");
    }

    report.section("Checking versions...");
    let local_version = match (&local, &config) {
        (Some(meta), Some(config)) if meta.profile != config.profile => {
            report.line(
                "⚠",
                format!(
                    "Local metadata belongs to profile '{}', configured profile is '{}'",
                    meta.profile, config.profile
                ),
            );
            None
        }
        (Some(meta), _) => Some(meta.version),
        _ => None,
    };
    let local_text = local_version
        .map(|v| v.to_string())
        .unwrap_or_else(|| "unknown".into());
    let remote_text = match remote_version {
        Some(Some(v)) => v.to_string(),
        Some(None) => "0 (no snapshots)".into(),
        None => "unknown".into(),
    };
    let status = match (local_version, remote_version) {
        (Some(l), Some(r)) if l == r.unwrap_or(0) => "in sync",
        (Some(l), Some(r)) if l < r.unwrap_or(0) => "remote ahead — run `opendotsync pull`",
        (Some(_), Some(_)) => "local ahead",
        _ => "comparison unavailable",
    };
    report.line(
        if status == "in sync" { "✓" } else { "⚠" },
        format!("Local version: {local_text}  Remote version: {remote_text}  ({status})"),
    );
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(root: &Path) {
        std::fs::create_dir(root.join("store")).unwrap();
        std::fs::create_dir(root.join("state")).unwrap();
        std::fs::create_dir(root.join("home")).unwrap();
        std::fs::write(root.join("home/.bashrc"), "hello").unwrap();
        crate::config::write_default_config(
            &root.join("config.yaml"),
            &BackendConfig::Local {
                path: root.join("store"),
            },
            "default",
        )
        .unwrap();
        std::fs::write(
            root.join("paths.yaml"),
            "include: [ .bashrc, .missing ]\nexclude: []\n",
        )
        .unwrap();
        metadata::save_metadata_to(
            &metadata::create_initial("default"),
            &root.join("state/metadata.json"),
        )
        .unwrap();
    }

    fn run(root: &Path) -> DoctorReport {
        diagnose(
            &root.join("config.yaml"),
            &root.join("paths.yaml"),
            &root.join("state"),
            &root.join("home"),
        )
    }

    #[test]
    fn healthy_local_and_missing_file_warning_do_not_change_store_or_state() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fixture(root);
        let before = std::fs::read(root.join("state/metadata.json")).unwrap();
        let report = run(root);
        assert!(!report.has_errors, "{report}");
        let output = report.to_string();
        assert!(
            output.contains("1 files matched (5 B projected)"),
            "{output}"
        );
        assert!(output.contains(".missing listed but file not found"));
        assert!(output.contains("in sync"));
        assert_eq!(std::fs::read_dir(root.join("store")).unwrap().count(), 0);
        assert_eq!(
            std::fs::read(root.join("state/metadata.json")).unwrap(),
            before
        );
    }

    #[test]
    fn missing_setup_reports_all_sections_without_initializing() {
        let tmp = tempfile::tempdir().unwrap();
        let report = run(tmp.path());
        assert!(report.has_errors);
        let output = report.to_string();
        for section in [
            "Config file",
            "Paths file",
            "State dir missing",
            "Local metadata missing",
            "Checking backend",
            "Checking tracked paths",
            "Checking encryption",
            "comparison unavailable",
        ] {
            assert!(output.contains(section), "missing {section}: {output}");
        }
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    #[test]
    fn corrupt_config_does_not_hide_paths_and_metadata_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fixture(root);
        std::fs::write(root.join("config.yaml"), "backend: [").unwrap();
        std::fs::write(root.join("paths.yaml"), "include: [").unwrap();
        std::fs::write(root.join("state/metadata.json"), "invalid").unwrap();
        let report = run(root);
        assert!(report.has_errors);
        let output = report.to_string();
        assert!(output.contains("failed to parse config"));
        assert!(output.contains("failed to parse paths"));
        assert!(output.contains("Local metadata:"));
    }

    #[test]
    fn failed_backend_does_not_hide_bad_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fixture(root);
        let mut config = crate::config::load_config(Some(&root.join("config.yaml"))).unwrap();
        config.backend = BackendConfig::Local {
            path: root.join("missing-store"),
        };
        config.encryption.enabled = true;
        let identity = root.join("identity.age");
        config.encryption.identity = Some(identity.clone());
        std::fs::write(&identity, "# public key: age1fake\nAGE-SECRET-KEY-BAD\n").unwrap();
        crate::config::save_config(&root.join("config.yaml"), &config).unwrap();
        let report = run(root);
        assert!(report.has_errors);
        let output = report.to_string();
        assert!(output.contains("✗ Remote reachable"));
        assert!(output.contains("✗ Encryption identity usable"));
        assert!(output.contains("Remote version: unknown"));
        assert!(!root.join("missing-store").exists());
        encryption::generate_keypair(&identity).unwrap();
        assert!(run(root)
            .to_string()
            .contains("✓ Encryption identity usable"));
    }

    #[test]
    fn versions_and_profile_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fixture(root);
        LocalBackend::new(root.join("store"))
            .push("default", b"data", None)
            .unwrap();
        assert!(run(root).to_string().contains("remote ahead"));
        metadata::save_metadata_to(
            &metadata::create_initial("other"),
            &root.join("state/metadata.json"),
        )
        .unwrap();
        let output = run(root).to_string();
        assert!(output.contains("belongs to profile 'other'"));
        assert!(output.contains("Local version: unknown"));
    }
}
