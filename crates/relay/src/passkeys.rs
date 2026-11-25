use crate::{cookie_key, database_error, digest, random_key, secure_cookie, session_key, Relay};
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use webauthn_rs::prelude::{
    DiscoverableAuthentication, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential, Url, Uuid, Webauthn, WebauthnBuilder,
};
use webauthn_rs_proto::ResidentKeyRequirement;

use crate::accounts::{Account, SessionKey, SESSION_SECONDS};

const CEREMONY_COOKIE: &str = "__Host-codex-passkey";
const CEREMONY_SECONDS: u64 = 180;
const RECENT_LOGIN_SECONDS: u64 = 5 * 60;
const MAX_PENDING: usize = 512;

enum Ceremony {
    Create {
        session: SessionKey,
        state: PasskeyRegistration,
    },
    SignUp {
        account: Account,
        state: PasskeyRegistration,
    },
    SignIn(DiscoverableAuthentication),
}

pub(crate) struct Passkeys {
    webauthn: Webauthn,
    pending: Mutex<HashMap<SessionKey, (Instant, Ceremony)>>,
}

impl Passkeys {
    pub(crate) fn new(origin: &str) -> Option<Self> {
        let url = Url::parse(origin).ok()?;
        let domain = url.domain()?;
        if url.origin().ascii_serialization() != origin
            || !(url.scheme() == "https" || (url.scheme() == "http" && domain == "localhost"))
        {
            return None;
        }
        let webauthn = WebauthnBuilder::new(domain, &url)
            .ok()?
            .rp_name("Codex Mobile")
            .timeout(Duration::from_secs(CEREMONY_SECONDS))
            .build()
            .ok()?;
        Some(Self {
            webauthn,
            pending: Mutex::default(),
        })
    }

    fn begin(
        &self,
        headers: &HeaderMap,
        ceremony: Ceremony,
        options: impl Serialize,
    ) -> Result<Response, StatusCode> {
        let now = Instant::now();
        let mut pending = self.pending.lock().unwrap();
        pending.retain(|_, (expires_at, _)| *expires_at > now);
        if let Some(old) = cookie_key(headers, CEREMONY_COOKIE) {
            pending.remove(&old);
        }
        if pending.len() >= MAX_PENDING {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        let key = random_key();
        pending.insert(
            digest(&key),
            (now + Duration::from_secs(CEREMONY_SECONDS), ceremony),
        );
        Ok((
            [
                (
                    header::SET_COOKIE,
                    secure_cookie(CEREMONY_COOKIE, &key, CEREMONY_SECONDS as i64),
                ),
                (header::CACHE_CONTROL, "private, no-store".parse().unwrap()),
            ],
            Json(options),
        )
            .into_response())
    }

    fn take(&self, headers: &HeaderMap) -> Result<Ceremony, StatusCode> {
        let key = cookie_key(headers, CEREMONY_COOKIE).ok_or(StatusCode::UNAUTHORIZED)?;
        // 先取走挑战，再验证签名，保证同一个挑战最多处理一次。
        self.pending
            .lock()
            .unwrap()
            .remove(&key)
            .filter(|(expires_at, _)| *expires_at > Instant::now())
            .map(|(_, ceremony)| ceremony)
            .ok_or(StatusCode::UNAUTHORIZED)
    }

    fn registration(
        &self,
        handle: Uuid,
        name: &str,
        existing: Option<Vec<webauthn_rs::prelude::CredentialID>>,
        platform: bool,
    ) -> Result<
        (
            webauthn_rs::prelude::CreationChallengeResponse,
            PasskeyRegistration,
        ),
        StatusCode,
    > {
        let (mut options, state) = self
            .webauthn
            .start_passkey_registration(handle, name, name, existing)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let selection = options
            .public_key
            .authenticator_selection
            .as_mut()
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        selection.resident_key = Some(ResidentKeyRequirement::Required);
        selection.require_resident_key = true;
        if platform {
            selection.authenticator_attachment =
                Some(webauthn_rs_proto::AuthenticatorAttachment::Platform);
        }
        Ok((options, state))
    }
}

fn allowed<'a>(relay: &'a Relay, headers: &HeaderMap) -> Result<&'a Passkeys, StatusCode> {
    if !relay.mobile_allowed(headers) {
        return Err(StatusCode::FORBIDDEN);
    }
    relay
        .passkeys
        .as_deref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

async fn create_options(
    State(relay): State<Relay>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let passkeys = allowed(&relay, &headers)?;
    let session = session_key(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let (account, remaining) = relay.mobile_session(session)?;
    if remaining < SESSION_SECONDS as u64 - RECENT_LOGIN_SECONDS {
        return Err(StatusCode::FORBIDDEN);
    }
    let handle = Uuid::parse_str(&account.id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let existing = relay
        .accounts
        .passkey_ids(&account.id)
        .map_err(database_error)?;
    let (options, state) = passkeys.registration(handle, &account.name, Some(existing), false)?;
    passkeys.begin(&headers, Ceremony::Create { session, state }, options)
}

fn require_signed_out(relay: &Relay, headers: &HeaderMap) -> Result<(), StatusCode> {
    if let Some(session) = session_key(headers) {
        if relay
            .accounts
            .session(session)
            .map_err(database_error)?
            .is_some()
        {
            return Err(StatusCode::CONFLICT);
        }
    }
    Ok(())
}

async fn sign_up_options(
    State(relay): State<Relay>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let passkeys = allowed(&relay, &headers)?;
    require_signed_out(&relay, &headers)?;
    let handle = Uuid::new_v4();
    let id = handle.simple().to_string();
    let account = Account {
        name: format!("Codex Mobile {}", &id[..12]),
        id,
    };
    let (options, state) = passkeys.registration(handle, &account.name, None, true)?;
    passkeys.begin(&headers, Ceremony::SignUp { account, state }, options)
}

async fn sign_up(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(credential): Json<RegisterPublicKeyCredential>,
) -> Result<Response, StatusCode> {
    let passkeys = allowed(&relay, &headers)?;
    require_signed_out(&relay, &headers)?;
    let Ceremony::SignUp { account, state } = passkeys.take(&headers)? else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let passkey = passkeys
        .webauthn
        .finish_passkey_registration(&credential, &state)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let key = random_key();
    relay
        .accounts
        .sign_up_with_passkey(&account, &passkey, digest(&key))
        .map_err(database_error)?;
    relay.signed_in(account, &key, None)
}

async fn create(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(credential): Json<RegisterPublicKeyCredential>,
) -> Result<StatusCode, StatusCode> {
    let passkeys = allowed(&relay, &headers)?;
    let Ceremony::Create { session, state } = passkeys.take(&headers)? else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    if session_key(&headers) != Some(session) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let passkey = passkeys
        .webauthn
        .finish_passkey_registration(&credential, &state)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if !relay
        .accounts
        .add_passkey(session, &passkey)
        .map_err(database_error)?
    {
        return Err(StatusCode::CONFLICT);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn sign_in_options(
    State(relay): State<Relay>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let passkeys = allowed(&relay, &headers)?;
    let (mut options, state) = passkeys
        .webauthn
        .start_discoverable_authentication()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    options.mediation = None;
    passkeys.begin(&headers, Ceremony::SignIn(state), options)
}

async fn sign_in(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(credential): Json<PublicKeyCredential>,
) -> Result<Response, StatusCode> {
    let passkeys = allowed(&relay, &headers)?;
    let Ceremony::SignIn(state) = passkeys.take(&headers)? else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let (handle, credential_id) = passkeys
        .webauthn
        .identify_discoverable_authentication(&credential)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let key = random_key();
    let previous = session_key(&headers);
    let account = relay
        .accounts
        .login_with_passkey(
            &handle.simple().to_string(),
            credential_id,
            |passkey| {
                passkeys
                    .webauthn
                    .finish_discoverable_authentication_v2(&credential, state, passkey)
                    .ok()
            },
            digest(&key),
            previous,
        )
        .map_err(database_error)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    relay.signed_in(account, &key, previous)
}

pub(crate) fn routes() -> Router<Relay> {
    Router::new()
        .route(
            "/relay/account/passkeys/create/options",
            post(create_options),
        )
        .route("/relay/account/passkeys/create", post(create))
        .route(
            "/relay/account/passkeys/sign-in/options",
            post(sign_in_options),
        )
        .route("/relay/account/passkeys/sign-in", post(sign_in))
        .route(
            "/relay/account/passkeys/sign-up/options",
            post(sign_up_options),
        )
        .route("/relay/account/passkeys/sign-up", post(sign_up))
        .layer(DefaultBodyLimit::max(64 * 1024))
}
