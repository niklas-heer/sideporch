//! How a new account signs in: a password, or a passkey made while filling
//! in the setup, invite or sign-up form.
//!
//! A passkey account starts like this: the form asks
//! `/webauthn/create/options` for a ceremony bound to that form, name and
//! username, the browser makes the passkey, and app.js sends the form with
//! the answer instead of a password. The form's own handler then checks
//! everything it always checks (setup code, invite, proof of work) and
//! creates the account and its passkey in one transaction. A passkey
//! counts as both sign-in steps, so the new account meets every policy.

use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::json;

use super::{
    Setup, check_names,
    security::{passkey_site, site},
};
use crate::{
    AppState,
    access::{self, ClientIp, Counted},
    community::{self, Joining, Registration},
    error::{AppError, AppResult},
    now_ms,
    passkeys::{self, NewPasskey, Purpose},
    security, store,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/webauthn/create/options", post(create_options))
}

/// What a new account will sign in with, checked and ready to store.
pub enum Credential {
    /// A password's hash.
    Password(String),
    Passkey {
        passkey: NewPasskey,
        handle: Vec<u8>,
        name: String,
    },
}

impl Credential {
    /// The password hash to store: none for passkey accounts, which
    /// `auth::verify_password` never accepts.
    pub fn password_hash(&self) -> &str {
        match self {
            Self::Password(hash) => hash,
            Self::Passkey { .. } => "",
        }
    }

    /// Stores the passkey of an account just created, in the same
    /// transaction, so a failure leaves no account behind.
    pub fn attach(&self, conn: &Connection, user_id: i64, now: i64) -> AppResult<()> {
        if let Self::Passkey {
            passkey,
            handle,
            name,
        } = self
        {
            conn.execute(
                "UPDATE users SET webauthn_handle = ?1 WHERE id = ?2",
                params![handle, user_id],
            )?;
            security::add_passkey(conn, user_id, passkey, name, now)?;
        }
        Ok(())
    }
}

/// What a form sends to make a new account: a password, or a passkey
/// answer as JSON.
pub enum Secret {
    Password(String),
    Passkey(String),
}

/// Hashes the password, or checks that the passkey was made for this
/// `form`, name and username. `Err(BadRequest)` carries a message for the
/// form.
pub async fn prepare(
    state: &AppState,
    headers: &HeaderMap,
    form: &str,
    username: &str,
    display_name: &str,
    secret: Secret,
) -> AppResult<Credential> {
    let answer = match secret {
        Secret::Password(password) => {
            return Ok(Credential::Password(
                crate::auth::hash_password(password).await?,
            ));
        }
        Secret::Passkey(answer) => answer,
    };
    let registration: passkeys::Registration = serde_json::from_str(&answer)
        .map_err(|_| AppError::bad_request("The passkey answer was damaged. Try again."))?;
    let (challenge, purpose) =
        state
            .ceremonies
            .finish_if(&registration.ceremony, now_ms(), |purpose| {
                matches!(purpose, Purpose::Create { form: f, username: u, display_name: d, .. }
                if f == form && u == username && d == display_name)
            })?;
    let Purpose::Create { handle, .. } = purpose else {
        return Err(AppError::internal("a passkey ceremony changed its purpose"));
    };
    let passkey = passkeys::verify_registration(&registration, &challenge, &site(state, headers))?;
    let name: String = registration.name.trim().chars().take(60).collect();
    let name = if name.is_empty() {
        passkeys::default_name(&passkey.transports)
    } else {
        name
    };
    Ok(Credential::Passkey {
        passkey,
        handle,
        name,
    })
}

/// Whether `form` can make a new account right now: setup isn't done, the
/// invite works, or anyone may sign up without approval. Approved sign-ups
/// wait in a queue that keeps a password, so they use one.
/// Behind Cloudflare Access, setup also needs a person Access let in.
async fn form_open(state: &AppState, headers: &HeaderMap, form: &str) -> AppResult<bool> {
    if form == "/setup" || form.starts_with("/setup/") {
        let link_fits = form
            .strip_prefix("/setup/")
            .is_none_or(|token| state.setup_link_matches(token));
        return Ok(state.setup() != Setup::Done
            && link_fits
            && crate::cloudflare::setup_gate(state, headers).await.is_ok());
    }
    if form == "/signup" {
        let joining = state.db.call(|conn| Joining::load(conn)).await?;
        return Ok(joining.registration == Registration::Open);
    }
    if let Some(token) = form.strip_prefix("/join/") {
        let token = token.to_owned();
        let now = now_ms();
        return state
            .db
            .call(move |conn| store::invite_is_valid(conn, &token, now))
            .await;
    }
    Ok(false)
}

/// Checks the setup code, counting wrong ones per address like `/setup`.
fn check_setup_code(
    state: &AppState,
    address: Option<std::net::IpAddr>,
    code: &str,
) -> AppResult<()> {
    let counted = address.map(Counted::SetupCode);
    if let Some(wait) = counted
        .as_ref()
        .and_then(|counted| state.access.check(counted, access::SETUP_CODE).err())
    {
        return Err(AppError::bad_request(format!(
            "Too many wrong setup codes. Try again in {}.",
            access::wait_text(wait)
        )));
    }
    if state.setup_code_matches(code) {
        return Ok(());
    }
    if let Some(counted) = counted {
        state.access.count(counted);
    }
    Err(AppError::bad_request("That isn't the setup code."))
}

#[derive(Deserialize)]
struct CreateOptions {
    form: String,
    display_name: String,
    username: String,
    /// Checked here too, so a wrong code doesn't leave a passkey behind
    /// that has no account.
    #[serde(default)]
    setup_code: String,
}

/// Starts making a passkey for a new account. Anyone can call this, so
/// it's limited per address, and only works for a form that is open.
async fn create_options(
    State(state): State<AppState>,
    ClientIp(address): ClientIp,
    headers: HeaderMap,
    Json(input): Json<CreateOptions>,
) -> AppResult<Json<serde_json::Value>> {
    if let Some(address) = address {
        let counted = Counted::PasskeyCreate(address);
        if let Err(wait) = state.access.check(&counted, access::PASSKEY_CREATE) {
            return Err(AppError::bad_request(format!(
                "Too many tries. Try again in {}.",
                access::wait_text(wait)
            )));
        }
        state.access.count(counted);
    }
    if !form_open(&state, &headers, &input.form).await? {
        return Err(AppError::bad_request(
            "This page can't make new accounts anymore. Reload it.",
        ));
    }
    if input.form == "/setup" && matches!(state.setup(), Setup::Secret { .. }) {
        check_setup_code(&state, address, &input.setup_code)?;
    }
    let (display_name, username) =
        check_names(&input.display_name, &input.username).map_err(AppError::bad_request)?;
    let lookup = username.clone();
    let taken = state
        .db
        .call(move |conn| community::username_taken(conn, &lookup))
        .await?;
    if taken {
        return Err(AppError::bad_request(
            "That username is taken. Try another one.",
        ));
    }
    let site = passkey_site(&state, &headers)?;
    let mut handle = vec![0_u8; 32];
    getrandom::fill(&mut handle).map_err(AppError::internal)?;
    let (ceremony, challenge) = state.ceremonies.start(
        Purpose::Create {
            form: input.form,
            username: username.clone(),
            display_name: display_name.clone(),
            handle: handle.clone(),
        },
        now_ms(),
    )?;
    Ok(Json(json!({
        "ceremony": ceremony,
        "publicKey": {
            "challenge": passkeys::encode(&challenge),
            "rp": { "name": "Sideporch", "id": site.rp_id },
            "user": { "id": passkeys::encode(&handle), "name": username, "displayName": display_name },
            "pubKeyCredParams": passkeys::ALGORITHMS.iter().map(|alg| json!({ "type": "public-key", "alg": alg })).collect::<Vec<_>>(),
            "timeout": 120_000,
            "attestation": "none",
            "excludeCredentials": [],
            "authenticatorSelection": { "residentKey": "required", "requireResidentKey": true, "userVerification": "required" },
        },
    })))
}
