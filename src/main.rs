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

        cli::Command::Push { force, message } => {
            let config = config::load_config(config_path)?;
            let paths_list = paths::load_paths(None)?;
            let version =
                sync::cmd_push(&config, &paths_list, home_dir, force, message.as_deref())?;
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

        _ => {
            eprintln!("Command not yet implemented");
            std::process::exit(1);
        }
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

    let binary = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("opendotsync"));

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

    let binary = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("opendotsync"));

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
