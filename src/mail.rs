//! Email through an SMTP server the admin sets up: sign-in links, password
//! resets and confirming addresses. Without it, Sideporch works as before.

use lettre::{
    AsyncSmtpTransport, AsyncTransport as _, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use rusqlite::Connection;

use crate::{
    error::{AppError, AppResult},
    secrets::Vault,
    store,
};

/// How to reach the mail server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Security {
    /// Plain connection upgraded with STARTTLS, usually port 587.
    #[default]
    StartTls,
    /// TLS from the start, usually port 465.
    Tls,
    /// No encryption, for a relay on the same machine or network.
    None,
}

impl Security {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "starttls" => Some(Self::StartTls),
            "tls" => Some(Self::Tls),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::StartTls => "starttls",
            Self::Tls => "tls",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub security: Security,
    pub username: String,
    /// Kept sealed with the secret key.
    pub password: Option<String>,
    /// Who mails come from, like `Sideporch <chat@example.com>`.
    pub from: String,
}

const PASSWORD_NAME: &str = "mail.password";

impl Settings {
    pub fn load(conn: &Connection, vault: &Vault) -> AppResult<Self> {
        let text =
            |key: &str| -> AppResult<String> { Ok(store::setting(conn, key)?.unwrap_or_default()) };
        let password = match store::setting(conn, "mail.password")? {
            Some(sealed) if !sealed.is_empty() => Some(vault.open_text(PASSWORD_NAME, &sealed)?),
            _ => None,
        };
        Ok(Self {
            host: text("mail.host")?,
            port: text("mail.port")?.parse().unwrap_or(587),
            security: Security::parse(&text("mail.security")?).unwrap_or_default(),
            username: text("mail.username")?,
            password,
            from: text("mail.from")?,
        })
    }

    /// Saves the settings; `password` of `None` keeps the stored one.
    pub fn save(&self, conn: &Connection, vault: &Vault, password: Option<&str>) -> AppResult<()> {
        if !self.host.is_empty() {
            if self.host.contains(char::is_whitespace) || self.host.contains('/') {
                return Err(AppError::bad_request(
                    "Enter the mail server's host name, like smtp.example.com.",
                ));
            }
            self.from.parse::<Mailbox>().map_err(|_| {
                AppError::bad_request("Enter the sender like Sideporch <chat@example.com>.")
            })?;
        }
        store::set_setting(conn, "mail.host", self.host.trim())?;
        store::set_setting(conn, "mail.port", &self.port.to_string())?;
        store::set_setting(conn, "mail.security", self.security.key())?;
        store::set_setting(conn, "mail.username", self.username.trim())?;
        store::set_setting(conn, "mail.from", self.from.trim())?;
        match password {
            Some("") => store::set_setting(conn, "mail.password", "")?,
            Some(password) => {
                store::set_setting(
                    conn,
                    "mail.password",
                    &vault.seal_text(PASSWORD_NAME, password)?,
                )?;
            }
            None => {}
        }
        Ok(())
    }

    pub const fn configured(&self) -> bool {
        !self.host.is_empty() && !self.from.is_empty()
    }
}

/// Whether email is set up, without decrypting anything.
pub fn configured(conn: &Connection) -> AppResult<bool> {
    Ok(
        store::setting(conn, "mail.host")?.is_some_and(|host| !host.is_empty())
            && store::setting(conn, "mail.from")?.is_some_and(|from| !from.is_empty()),
    )
}

/// Sends one plain-text email.
pub async fn send(settings: &Settings, to: &str, subject: &str, body: &str) -> AppResult<()> {
    if !settings.configured() {
        return Err(AppError::bad_request(
            "Email isn't set up on this Sideporch.",
        ));
    }
    let from: Mailbox = settings.from.parse().map_err(|_| {
        AppError::bad_request("The sender address in the email settings is invalid.")
    })?;
    let to: Mailbox = to
        .parse()
        .map_err(|_| AppError::bad_request("That email address doesn't look right."))?;
    let message = Message::builder()
        .from(from)
        .to(to)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_owned())
        .map_err(AppError::internal)?;
    let builder = match settings.security {
        Security::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&settings.host),
        Security::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&settings.host),
        Security::None => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
            &settings.host,
        )),
    }
    .map_err(|error| {
        AppError::bad_request(format!("The mail server settings don't work: {error}"))
    })?;
    let mut builder = builder
        .port(settings.port)
        .timeout(Some(std::time::Duration::from_secs(20)));
    if !settings.username.is_empty() {
        builder = builder.credentials(Credentials::new(
            settings.username.clone(),
            settings.password.clone().unwrap_or_default(),
        ));
    }
    builder.build().send(message).await.map_err(|error| {
        AppError::bad_request(format!("The mail server refused the email: {error}"))
    })?;
    Ok(())
}

/// Whether text is a plausible email address, to store.
pub fn valid_address(address: &str) -> bool {
    address.len() <= 254 && address.parse::<lettre::Address>().is_ok()
}
