#![allow(dead_code)]
use clap::Parser;

mod backend;
mod cli;
mod collector;
mod config;
mod conflicts;
mod encryption;
mod merge;
mod metadata;
mod paths;
mod profiles;
mod retention;
mod snapshot;
mod sync;

fn main() {
    env_logger::init();
    let cli = cli::Cli::parse();
    backend::set_verbose(cli.verbose);

    let home_dir = cli
        .home
        .unwrap_or_else(|| dirs::home_dir().expect("could not determine home directory"));

    let result = run(cli.command, cli.config.as_deref(), &home_dir, cli.quiet);

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn run(
    command: cli::Command,
    config_path: Option<&std::path::Path>,
    home_dir: &std::path::Path,
    quiet: bool,
) -> Result<(), sync::SyncError> {
    match command {
        cli::Command::Init {
            backend,
            profile,
            path,
        } => sync::cmd_init(home_dir, &backend, path.as_deref(), &profile),

        cli::Command::Push {
            force,
            message,
            no_auto_pull,
        } => {
            let config = config::load_config(config_path)?;
            let paths_list = paths::load_paths(None)?;
            let version = sync::cmd_push(
                &config,
                &paths_list,
                home_dir,
                force,
                message.as_deref(),
                no_auto_pull,
            )?;
            if !quiet {
                println!("Pushed version {version}");
            }
            Ok(())
        }

        cli::Command::Pull { version, overwrite } => {
            let config = config::load_config(config_path)?;
            let paths_list = paths::load_paths(None)?;
            let result = sync::cmd_pull(&config, &paths_list, home_dir, version, overwrite)?;
            if !quiet {
                print!("{result}");
            }
            Ok(())
        }

        cli::Command::Status => {
            let config = config::load_config(config_path)?;
            let paths_list = paths::load_paths(None)?;
            let report = sync::cmd_status(&config, &paths_list, home_dir)?;
            print!("{report}");
            Ok(())
        }

        cli::Command::Du => {
            let paths_list = paths::load_paths(None)?;
            let report = sync::cmd_du(&paths_list, home_dir)?;
            print!("{report}");
            Ok(())
        }

        cli::Command::Sync { dry_run, force } => {
            let config = config::load_config(config_path)?;
            let paths_list = paths::load_paths(None)?;
            sync::cmd_sync(&config, &paths_list, home_dir, dry_run, force, quiet)
        }

        cli::Command::InstallTimer { uninstall } => install_timer(uninstall),

        cli::Command::EncryptKey { action } => match action {
            cli::EncryptKeyAction::Generate => {
                let identity_path = config::default_config_dir().join("identity.age");
                let pubkey = encryption::generate_keypair(&identity_path)?;
                if !quiet {
                    println!("Generated key pair");
                    println!("  Identity file: {}", identity_path.display());
                    println!("  Public key: {pubkey}");
                    println!();
                    println!("Add to ~/.config/opendotsync/config.yaml:");
                    println!("  encryption:");
                    println!("    enabled: true");
                    println!("    identity: {}", identity_path.display());
                }
                Ok(())
            }
            cli::EncryptKeyAction::Export => {
                let config = config::load_config(config_path)?;
                let identity_path = config.encryption.identity.ok_or_else(|| {
                    sync::SyncError::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "no identity configured in config.yaml",
                    ))
                })?;
                let pubkey = encryption::export_public_key(&identity_path)?;
                println!("{pubkey}");
                Ok(())
            }
        },

        cli::Command::Profiles { action } => {
            let config = config::load_config(config_path)?;
            match action {
                cli::ProfilesAction::List => {
                    let profiles = profiles::list_profiles(&config)?;
                    let current = profiles::current_profile(&config);
                    if profiles.is_empty() {
                        println!("No profiles found (run `push` to create one)");
                    } else {
                        for p in &profiles {
                            let marker = if p == current { " *" } else { "" };
                            println!("{p}{marker}");
                        }
                    }
                    Ok(())
                }
                cli::ProfilesAction::Create { name } => {
                    profiles::create_profile(&name)?;
                    println!("Profile '{name}' ready — run `push` to populate it");
                    Ok(())
                }
                cli::ProfilesAction::Switch { name } => {
                    let config_path = config::default_config_path();
                    profiles::switch_profile(&config_path, &name, &config)?;
                    println!("Switched to profile '{name}'");
                    Ok(())
                }
                cli::ProfilesAction::Merge { from, to } => {
                    sync::cmd_profiles_merge(&config, &from, &to)?;
                    if !quiet {
                        println!("Merged profile '{from}' into '{to}'");
                    }
                    Ok(())
                }
            }
        }

        cli::Command::Log { n } => {
            let config = config::load_config(config_path)?;
            let versions = sync::cmd_log(&config, n)?;
            if versions.is_empty() {
                println!("No snapshots yet.");
            }
            for v in &versions {
                let msg = v.message.as_deref().unwrap_or("");
                println!(
                    "v{:03}  {}  {:>8}  {}",
                    v.version,
                    v.timestamp.format("%Y-%m-%d %H:%M"),
                    format_size(v.size_bytes),
                    msg,
                );
            }
            Ok(())
        }

        cli::Command::Diff { remote } => {
            let config = config::load_config(config_path)?;
            let paths_list = paths::load_paths(None)?;
            let (modified, added, deleted) =
                sync::cmd_diff(&config, &paths_list, home_dir, remote)?;
            if modified.is_empty() && added.is_empty() && deleted.is_empty() {
                println!("No changes.");
            } else {
                for p in &modified {
                    println!("  M {p}");
                }
                for p in &added {
                    println!("  A {p}");
                }
                for p in &deleted {
                    println!("  D {p}");
                }
            }
            Ok(())
        }

        cli::Command::Restore { version, path } => {
            let config = config::load_config(config_path)?;
            let restored = sync::cmd_restore(&config, home_dir, version, path.as_deref())?;
            if !quiet {
                for p in &restored {
                    println!("  restored {p}");
                }
                println!("Restored {} file(s) from v{version}", restored.len());
            }
            Ok(())
        }

        cli::Command::Conflicts { action } => match action {
            cli::ConflictsAction::List => {
                let entries = sync::cmd_conflicts_list()?;
                if entries.is_empty() {
                    println!("No conflicts.");
                }
                for (v, files) in &entries {
                    println!("v{v}:");
                    for f in files {
                        println!("  {f}");
                    }
                }
                Ok(())
            }
            cli::ConflictsAction::Show { path } => {
                let (local, remote) = sync::cmd_conflicts_show(home_dir, &path)?;
                println!("=== LOCAL ===");
                println!("{}", String::from_utf8_lossy(&local));
                println!("=== REMOTE (stashed) ===");
                println!("{}", String::from_utf8_lossy(&remote));
                Ok(())
            }
            cli::ConflictsAction::AcceptLocal { path } => {
                sync::cmd_conflicts_accept_local(&path)?;
                println!("Kept local version of '{path}', removed conflict stash.");
                Ok(())
            }
            cli::ConflictsAction::AcceptRemote { path } => {
                sync::cmd_conflicts_accept_remote(home_dir, &path)?;
                println!("Replaced local '{path}' with remote version.");
                Ok(())
            }
            cli::ConflictsAction::Resolve => {
                let entries = sync::cmd_conflicts_list()?;
                if entries.is_empty() {
                    println!("No conflicts to resolve.");
                    return Ok(());
                }
                println!("Unresolved conflicts:");
                for (v, files) in &entries {
                    for f in files {
                        println!("  v{v}: {f}");
                    }
                }
                println!();
                println!("Use `conflicts show <path>` to inspect, then");
                println!("`conflicts accept-local <path>` or `conflicts accept-remote <path>`.");
                Ok(())
            }
        },

        cli::Command::Config { action } => match action {
            cli::ConfigAction::Show => {
                let path = config_path
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(config::default_config_path);
                if !path.exists() {
                    return Err(sync::SyncError::NotInitialized);
                }
                print!("{}", std::fs::read_to_string(&path)?);
                Ok(())
            }
            cli::ConfigAction::Edit => {
                let path = config_path
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(config::default_config_path);
                let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
                std::process::Command::new(&editor).arg(&path).status()?;
                Ok(())
            }
        },

        cli::Command::Paths { action } => match action {
            cli::PathsAction::List => {
                let p = paths::default_paths_path();
                if !p.exists() {
                    return Err(sync::SyncError::NotInitialized);
                }
                let list = paths::load_paths(Some(&p))?;
                println!("Allowlist (synced):");
                if list.include.is_empty() {
                    println!("  (none)");
                } else {
                    for entry in &list.include {
                        let suffix = match entry.merge_type {
                            paths::MergeType::None => {
                                if entry.path.ends_with('/') {
                                    "  (dir)".to_string()
                                } else {
                                    String::new()
                                }
                            }
                            paths::MergeType::History => "  [merge: history]".to_string(),
                            paths::MergeType::Union => "  [merge: union]".to_string(),
                        };
                        println!("  {}{}", entry.path, suffix);
                    }
                }
                println!("\nBlacklist (never synced):");
                if list.exclude.is_empty() {
                    println!("  (none)");
                } else {
                    for pattern in &list.exclude {
                        println!("  {pattern}");
                    }
                }
                Ok(())
            }
            cli::PathsAction::Edit => {
                let p = paths::default_paths_path();
                let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
                std::process::Command::new(&editor).arg(&p).status()?;
                Ok(())
            }
            cli::PathsAction::Add { path, merge_type } => {
                let p = paths::default_paths_path();
                let mut list = paths::load_paths(Some(&p))?;
                let mt = match merge_type.as_str() {
                    "history" => paths::MergeType::History,
                    "union" => paths::MergeType::Union,
                    _ => paths::MergeType::None,
                };
                list.include.push(paths::PathEntry {
                    path: path.clone(),
                    merge_type: mt,
                });
                paths::save_paths(&p, &list)?;
                println!("Added '{path}'");
                Ok(())
            }
            cli::PathsAction::Remove { path } => {
                let p = paths::default_paths_path();
                let mut list = paths::load_paths(Some(&p))?;
                let before = list.include.len();
                list.include.retain(|e| e.path != path);
                if list.include.len() == before {
                    eprintln!("'{path}' not found in paths list");
                } else {
                    paths::save_paths(&p, &list)?;
                    println!("Removed '{path}'");
                }
                Ok(())
            }
            cli::PathsAction::Exclude { action } => match action {
                cli::ExcludeAction::List => {
                    let p = paths::default_paths_path();
                    let list = paths::load_paths(Some(&p))?;
                    if list.exclude.is_empty() {
                        println!("Blacklist is empty");
                    } else {
                        for pattern in &list.exclude {
                            println!("{pattern}");
                        }
                    }
                    Ok(())
                }
                cli::ExcludeAction::Add { pattern } => {
                    let p = paths::default_paths_path();
                    let mut list = paths::load_paths(Some(&p))?;
                    if list.exclude.iter().any(|e| e == &pattern) {
                        eprintln!("'{pattern}' already in blacklist");
                    } else {
                        list.exclude.push(pattern.clone());
                        paths::save_paths(&p, &list)?;
                        println!("Excluded '{pattern}'");
                    }
                    Ok(())
                }
                cli::ExcludeAction::Remove { pattern } => {
                    let p = paths::default_paths_path();
                    let mut list = paths::load_paths(Some(&p))?;
                    let before = list.exclude.len();
                    list.exclude.retain(|e| e != &pattern);
                    if list.exclude.len() == before {
                        eprintln!("'{pattern}' not found in blacklist");
                    } else {
                        paths::save_paths(&p, &list)?;
                        println!("Removed '{pattern}' from blacklist");
                    }
                    Ok(())
                }
            },
            cli::PathsAction::Check { path } => {
                let p = paths::default_paths_path();
                let list = paths::load_paths(Some(&p))?;
                // Normalize: strip a leading "~/" or home prefix and leading slash
                // so users can pass ".bashrc", "~/.bashrc", or "/home/me/.bashrc".
                let rel = if let Ok(stripped) = std::path::Path::new(&path).strip_prefix(home_dir) {
                    stripped.to_string_lossy().to_string()
                } else {
                    path.trim_start_matches("~/")
                        .trim_start_matches('/')
                        .to_string()
                };
                match paths::check_path(&list, &rel) {
                    paths::SyncDecision::Synced {
                        matched,
                        merge_type,
                    } => {
                        let mt = match merge_type {
                            paths::MergeType::None => "none",
                            paths::MergeType::History => "history",
                            paths::MergeType::Union => "union",
                        };
                        println!("'{rel}' WILL be synced");
                        println!("  matched allowlist entry: '{matched}'");
                        println!("  merge strategy: {mt}");
                    }
                    paths::SyncDecision::Blocked { pattern } => {
                        println!("'{rel}' will NOT be synced");
                        println!("  blocked by blacklist pattern: '{pattern}'");
                    }
                    paths::SyncDecision::NotTracked => {
                        println!("'{rel}' will NOT be synced");
                        println!(
                            "  not on the allowlist (add it with 'opendotsync paths add {rel}')"
                        );
                    }
                }
                Ok(())
            }
        },
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn install_timer(uninstall: bool) -> Result<(), sync::SyncError> {
    #[cfg(target_os = "linux")]
    {
        install_systemd_timer(uninstall)
    }
    #[cfg(target_os = "macos")]
    {
        install_launchd_timer(uninstall)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = uninstall;
        eprintln!("install-timer is only supported on Linux (systemd) and macOS (launchd)");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
fn install_systemd_timer(uninstall: bool) -> Result<(), sync::SyncError> {
    let systemd_dir = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("~/.config"))
        .join("systemd/user");

    let service_dest = systemd_dir.join("opendotsync.service");
    let timer_dest = systemd_dir.join("opendotsync.timer");

    if uninstall {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "disable", "--now", "opendotsync.timer"])
            .status();
        let _ = std::fs::remove_file(&service_dest);
        let _ = std::fs::remove_file(&timer_dest);
        println!("Removed systemd timer");
        return Ok(());
    }

    std::fs::create_dir_all(&systemd_dir)?;

    let binary =
        std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("opendotsync"));

    let service_content = format!(
        "[Unit]\nDescription=OpenDotSync dotfile synchronization\n\n[Service]\nType=oneshot\nExecStart={} sync\n",
        binary.display()
    );
    let timer_content =
        "[Unit]\nDescription=OpenDotSync timer\n\n[Timer]\nOnBootSec=5min\nOnUnitActiveSec=15min\nRandomizedDelaySec=300\n\n[Install]\nWantedBy=timers.target\n";

    std::fs::write(&service_dest, service_content)?;
    std::fs::write(&timer_dest, timer_content)?;

    let status = std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", "opendotsync.timer"])
        .status()
        .map_err(sync::SyncError::Io)?;

    if status.success() {
        println!("Installed and enabled systemd timer (every 15 min)");
    } else {
        eprintln!("Warning: systemctl enable failed — timer files written but not activated");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn install_launchd_timer(uninstall: bool) -> Result<(), sync::SyncError> {
    let launch_agents = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("~"))
        .join("Library/LaunchAgents");

    let plist_dest = launch_agents.join("com.opendotsync.sync.plist");
    let label = "com.opendotsync.sync";

    if uninstall {
        let _ = std::process::Command::new("launchctl")
            .args(["unload", plist_dest.to_str().unwrap_or("")])
            .status();
        let _ = std::fs::remove_file(&plist_dest);
        println!("Removed launchd plist");
        return Ok(());
    }

    std::fs::create_dir_all(&launch_agents)?;

    let binary =
        std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("opendotsync"));

    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \t<key>Label</key><string>{label}</string>\n\
         \t<key>ProgramArguments</key>\n\t<array>\n\
         \t\t<string>{bin}</string>\n\
         \t\t<string>sync</string>\n\
         \t</array>\n\
         \t<key>StartInterval</key><integer>900</integer>\n\
         \t<key>RunAtLoad</key><false/>\n\
         </dict>\n</plist>\n",
        label = label,
        bin = binary.display()
    );

    std::fs::write(&plist_dest, plist)?;

    let status = std::process::Command::new("launchctl")
        .args(["load", plist_dest.to_str().unwrap_or("")])
        .status()
        .map_err(sync::SyncError::Io)?;

    if status.success() {
        println!("Installed launchd plist (every 15 min)");
    } else {
        eprintln!("Warning: launchctl load failed — plist written but not activated");
    }
    Ok(())
}
