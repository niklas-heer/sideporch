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
    /// A secret of your choice that the setup page asks for before anyone
    /// can create the first account, at least 16 characters. Easiest on
    /// platforms without a shell: set it as a secret, open the site, and
    /// type it in. Generate one with `openssl rand -base64 24`.
    #[arg(long, env = "SIDEPORCH_SETUP_CODE", hide_env_values = true)]
    setup_code: Option<String>,
    /// Ask GitHub for new releases every few hours, so admins hear about
    /// updates. `false` keeps Sideporch from contacting GitHub at all.
    #[arg(long, env = "SIDEPORCH_UPDATE_CHECK", default_value_t = true, action = clap::ArgAction::Set)]
    update_check: bool,
    /// Behind a reverse proxy, the header it puts the client's address in,
    /// such as `X-Forwarded-For` or `Fly-Client-IP`. Only set it when every
    /// request comes through that proxy; otherwise anyone could claim any
    /// address.
    #[arg(long, env = "SIDEPORCH_CLIENT_IP_HEADER")]
    client_ip_header: Option<String>,
    /// Sign people in through Cloudflare Access: the team domain, such as
    /// `myteam.cloudflareaccess.com`. Set it with --cloudflare-access-aud.
    #[arg(long, env = "SIDEPORCH_CLOUDFLARE_ACCESS_TEAM")]
    cloudflare_access_team: Option<String>,
    /// The audience (AUD) tag of the Cloudflare Access application in front
    /// of this server, from its overview in Cloudflare One.
    #[arg(long, env = "SIDEPORCH_CLOUDFLARE_ACCESS_AUD")]
    cloudflare_access_aud: Option<String>,
    /// Refuse pages to requests that didn't come through Cloudflare Access.
    /// The health check, webhooks, federation and MCP stay open.
    #[arg(long, env = "SIDEPORCH_CLOUDFLARE_ACCESS_REQUIRED")]
    cloudflare_access_required: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Print the link for creating the first account.
    ///
    /// The running server keeps it in a file in the data directory that only
    /// its user can read. With Docker: `docker exec <container> /sideporch setup-link`.
    /// It is only secret when the server runs with `--require-setup-link`.
    SetupLink,
    /// Print a one-time link for choosing a new password, for 24 hours.
    ///
    /// The way back in when someone, such as the only admin, lost their
    /// passkey or password. Works while the server runs. With Docker:
    /// `docker exec <container> /sideporch reset-link <username>`.
    ResetLink {
        /// The account's username.
        username: String,
        /// Also remove the account's passkeys and authenticator app, when
        /// those are what was lost. Without this, signing in still asks for
        /// them after the new password.
        #[arg(long)]
        reset_sign_in: bool,
    },
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
    /// Install the newest release in place of this program.
    ///
    /// Downloads the release for this system, checks that its checksums are
    /// signed with Sideporch's release key and that the download matches
    /// them, and replaces this program. The one that ran before is kept
    /// next to it as `sideporch.previous`. Restart Sideporch afterwards.
    /// Homebrew, Nix and container installs update the way they were
    /// installed instead.
    Update {
        /// Only say whether a newer release exists.
        #[arg(long)]
        check: bool,
        /// Install this version instead of the newest, such as 0.5.0.
        #[arg(long)]
        version: Option<String>,
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
        Some(Command::ResetLink {
            username,
            reset_sign_in,
        }) => {
            return print_reset_link(
                &args.data,
                args.public_url.as_deref(),
                username,
                *reset_sign_in,
            );
        }
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
        Some(Command::Update { check, version }) => {
            return update(*check, version.as_deref()).await;
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
        setup_code: args.setup_code,
        gif_api_base: None,
        allow_insecure_push: false,
        allow_private_link_previews: false,
        model_base_url: None,
        update_check: args.update_check,
        update_source: None,
        allow_private_federation: false,
        client_ip_header: args.client_ip_header,
        cloudflare_access_team: args.cloudflare_access_team,
        cloudflare_access_aud: args.cloudflare_access_aud,
        cloudflare_access_required: args.cloudflare_access_required,
        cloudflare_access_certs_url: None,
    })
    .await?;
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    let base = match app.public_url() {
        Some(url) => url.to_owned(),
        None => format!("http://{}", listener.local_addr()?),
    };
    tracing::info!(data = %args.data.display(), "Sideporch is listening on {base}");
    if app.setup_needs_code() {
        tracing::info!(
            "No account exists yet. Open {base}/setup and enter the setup code to create the admin account"
        );
    } else if let Some(file) = app.save_setup_link(&base)? {
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
    axum::serve(listener, app.service())
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

/// `sideporch update`: checks for a newer release and installs it.
async fn update(check_only: bool, wanted: Option<&str>) -> ExitCode {
    use sideporch::updates::{Source, Updates, Version};
    let updates = match Updates::new(Source::default(), true, std::env::current_exe().ok(), false) {
        Ok(updates) => std::sync::Arc::new(updates),
        Err(error) => {
            eprintln!("Could not start updating: {error}");
            return ExitCode::FAILURE;
        }
    };
    let current = Version::current();
    let releases = match updates.fetch_releases().await {
        Ok(releases) => releases,
        Err(error) => {
            eprintln!("Could not check for updates: {error}");
            return ExitCode::FAILURE;
        }
    };
    let chosen = match wanted {
        Some(text) => {
            let Some(version) = Version::parse(text) else {
                eprintln!("{text} isn't a version; write it like 0.5.0.");
                return ExitCode::FAILURE;
            };
            let Some(release) = releases.iter().find(|release| release.version == version) else {
                eprintln!("There is no release {version}.");
                return ExitCode::FAILURE;
            };
            release
        }
        None => match releases.first() {
            Some(release) if release.version > current => release,
            _ => {
                println!("Sideporch {current} is up to date.");
                return ExitCode::SUCCESS;
            }
        },
    };
    let security = releases.iter().any(|release| {
        release.security && release.version > current && release.version <= chosen.version
    });
    println!(
        "Sideporch {}{} is available; this is {current}. What changed: {}",
        chosen.version,
        if security {
            ", with security fixes,"
        } else {
            ""
        },
        chosen.url
    );
    if check_only {
        return ExitCode::SUCCESS;
    }
    if let Some(reason) = updates.cannot_install() {
        eprintln!("{reason} {}", updates.method().advice());
        return ExitCode::FAILURE;
    }
    match updates.install(chosen.version).await {
        Ok(()) => {
            let path = updates
                .executable()
                .map_or_else(String::new, |path| path.display().to_string());
            println!(
                "Installed Sideporch {} as {path} (the version before is {path}.previous). Restart Sideporch to use it, for example with `sudo systemctl restart sideporch`.",
                chosen.version
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Could not update: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `sideporch reset-link`: prints a reset link, and says when signing in
/// will still ask for a passkey or app.
fn print_reset_link(
    data: &std::path::Path,
    public_url: Option<&str>,
    username: &str,
    reset_sign_in: bool,
) -> ExitCode {
    let link = match sideporch::reset_link(data, username, reset_sign_in) {
        Ok(link) => link,
        Err(error) => {
            eprintln!("Could not make a reset link: {error}");
            return ExitCode::FAILURE;
        }
    };
    let base = public_url.unwrap_or("").trim_end_matches('/');
    println!("{base}{}", link.path);
    if base.is_empty() {
        eprintln!("Open this path on your Sideporch's address.");
    }
    eprintln!("It works once, for 24 hours.");
    if reset_sign_in {
        eprintln!(
            "Their passkeys and authenticator app are removed; they can add new ones after signing in."
        );
    } else if link.second_step {
        eprintln!(
            "Signing in will still ask for their passkey or authenticator app. If those are lost, run this again with --reset-sign-in."
        );
    }
    ExitCode::SUCCESS
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
