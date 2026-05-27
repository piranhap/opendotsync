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

        _ => {
            eprintln!("Command not yet implemented");
            std::process::exit(1);
        }
    }
}
