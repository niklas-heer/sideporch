use std::{io::IsTerminal as _, net::SocketAddr, path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use sideporch::{Config, Sideporch};
use tracing_subscriber::EnvFilter;

/// A small, self-hosted team chat.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// Address and port to listen on.
    #[arg(long, env = "SIDEPORCH_LISTEN", default_value = "127.0.0.1:8080")]
    listen: SocketAddr,
    /// Directory for the database. Back up this directory to back up everything.
    #[arg(
        long,
        env = "SIDEPORCH_DATA",
        default_value = "sideporch-data",
        global = true
    )]
    data: PathBuf,
    /// Public URL people use to reach this server, such as `https://chat.example.com`.
    /// Used in invite and webhook links; secure cookies are enabled for https.
    #[arg(long, env = "SIDEPORCH_PUBLIC_URL")]
    public_url: Option<String>,
    /// Require a one-time link, printed by `sideporch setup-link`, to create
    /// the first account. Without it, the first visitor becomes the admin.
    #[arg(long, env = "SIDEPORCH_REQUIRE_SETUP_LINK")]
    require_setup_link: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Print the link for creating the first account.
    ///
    /// The running server keeps it in a file in the data directory that only
    /// its user can read. With Docker: `docker exec <container> /sideporch setup-link`.
    /// It is only secret when the server runs with `--require-setup-link`.
    SetupLink,
    /// Unpack a backup from Admin → Backups into the data directory.
    ///
    /// Also takes a `.db` copy from `upgrade-backups/`, which Sideporch keeps
    /// before each upgrade, for going back to the version you ran before.
    ///
    /// Stop the server first. The data directory must not hold a database
    /// yet, unless you pass --force to replace it.
    Restore {
        /// The backup archive, a `sideporch-….tar.gz` file, or a `.db` copy.
        archive: PathBuf,
        /// Replace the database that is already there.
        #[arg(long)]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();
    match &args.command {
        Some(Command::SetupLink) => return print_setup_link(&args.data),
        Some(Command::Restore { archive, force }) => {
            return match sideporch::restore(archive, &args.data, *force) {
                Ok(count) => {
                    println!(
                        "Restored {count} files into {}. Start Sideporch with this data directory.",
                        args.data.display()
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("Could not restore: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        None => {}
    }
    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("{error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let app = Sideporch::open(Config {
        data_dir: args.data.clone(),
        public_url: args.public_url,
        require_setup_link: args.require_setup_link,
        gif_api_base: None,
        allow_insecure_push: false,
        allow_private_link_previews: false,
    })
    .await?;
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    let base = match app.public_url() {
        Some(url) => url.to_owned(),
        None => format!("http://{}", listener.local_addr()?),
    };
    tracing::info!(data = %args.data.display(), "Sideporch is listening on {base}");
    if let Some(file) = app.save_setup_link(&base)? {
        // A one-time link grants the first admin account. Show it on an
        // interactive terminal; keep it out of service and container logs.
        if (!app.setup_is_secret() || std::io::stdout().is_terminal())
            && let Some(path) = app.setup_path()
        {
            tracing::info!("No account exists yet. Open {base}{path} to create the admin account");
        } else {
            tracing::info!(
                file = %file.display(),
                "No account exists yet. Run `sideporch setup-link` to get the one-time setup link"
            );
        }
    }
    axum::serve(listener, app.router())
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

fn print_setup_link(data: &std::path::Path) -> ExitCode {
    match std::fs::read_to_string(sideporch::setup_link_file(data)) {
        Ok(link) => {
            print!("{link}");
            ExitCode::SUCCESS
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "No setup link in {}: the first account already exists, or the server hasn't started yet.",
                data.display()
            );
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("Could not read the setup link: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn shutdown() {
    let interrupt = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let terminate = async {
            if let Ok(mut signal) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                signal.recv().await;
            }
        };
        tokio::select! {
            _ = interrupt => {}
            () = terminate => {}
        }
    }
    #[cfg(not(unix))]
    drop(interrupt.await);
}
