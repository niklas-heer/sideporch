use std::{net::SocketAddr, path::PathBuf, process::ExitCode};

use clap::Parser;
use sideporch::{Config, Sideporch};
use tracing_subscriber::EnvFilter;

/// A small, self-hosted team chat.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Address and port to listen on.
    #[arg(long, env = "SIDEPORCH_LISTEN", default_value = "127.0.0.1:8080")]
    listen: SocketAddr,
    /// Directory for the database. Back up this directory to back up everything.
    #[arg(long, env = "SIDEPORCH_DATA", default_value = "sideporch-data")]
    data: PathBuf,
    /// Public URL people use to reach this server, such as `https://chat.example.com`.
    /// Used in invite and webhook links; secure cookies are enabled for https.
    #[arg(long, env = "SIDEPORCH_PUBLIC_URL")]
    public_url: Option<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    match run(Args::parse()).await {
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
    })
    .await?;
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    let base = match app.public_url() {
        Some(url) => url.to_owned(),
        None => format!("http://{}", listener.local_addr()?),
    };
    tracing::info!(data = %args.data.display(), "Sideporch is listening on {base}");
    if let Some(path) = app.setup_path() {
        tracing::info!("Create the first account at {base}{path}");
    }
    axum::serve(listener, app.router())
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
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
