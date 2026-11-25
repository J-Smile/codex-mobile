use super::{
    bearer_token, database_error, digest, session_key, valid_desktop_name, valid_id, valid_key,
    Relay, SessionKey,
};
use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const LIFETIME: Duration = Duration::from_secs(180);
const MAX_PENDING: usize = 512;

#[derive(Clone, Default)]
pub(crate) struct DesktopLinks(Arc<Mutex<HashMap<String, Offer>>>);

struct Offer {
    id: String,
    name: String,
    desktop_key: SessionKey,
    expires: Instant,
}

#[derive(Deserialize)]
struct Invitation {
    code: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Claim {
    code: String,
    desktop_id: String,
}

async fn offer(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(invitation): Json<Invitation>,
) -> Result<Response, StatusCode> {
    let token = bearer_token(&headers)
        .filter(|key| key.strip_prefix("cm_device_").is_some_and(valid_key))
        .ok_or(StatusCode::FORBIDDEN)?;
    if !valid_id(&id) || !valid_key(&invitation.code) || !valid_desktop_name(&invitation.name) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let desktop_key = digest(token);
    let now = Instant::now();
    let mut offers = relay.desktop_links.0.lock().unwrap();
    if relay
        .accounts
        .linked_desktop(desktop_key, &id)
        .map_err(database_error)?
    {
        offers.retain(|_, entry| entry.desktop_key != desktop_key);
        return Err(StatusCode::CONFLICT);
    }
    offers.retain(|_, entry| entry.expires > now && entry.desktop_key != desktop_key);
    if offers.len() >= MAX_PENDING {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    if offers.contains_key(&invitation.code) {
        return Err(StatusCode::CONFLICT);
    }
    offers.insert(
        invitation.code,
        Offer {
            id,
            name: invitation.name,
            desktop_key,
            expires: now + LIFETIME,
        },
    );
    Ok((
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(serde_json::json!({ "expiresIn": LIFETIME.as_secs() })),
    )
        .into_response())
}

async fn claim(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(input): Json<Claim>,
) -> Result<StatusCode, StatusCode> {
    if !relay.mobile_allowed(&headers) {
        return Err(StatusCode::FORBIDDEN);
    }
    let session = session_key(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    if !valid_key(&input.code) || !valid_id(&input.desktop_id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let lookup = URL_SAFE_NO_PAD.encode(digest(&input.code));
    let mut offers = relay.desktop_links.0.lock().unwrap();
    let offer = offers
        .get(&lookup)
        .filter(|offer| offer.id == input.desktop_id && offer.expires > Instant::now())
        .ok_or(StatusCode::GONE)?;
    let linked = relay
        .accounts
        .link_desktop(session, &offer.id, &offer.name, offer.desktop_key)
        .map_err(database_error)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    offers.remove(&lookup);
    if !linked {
        return Err(StatusCode::CONFLICT);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn routes() -> Router<Relay> {
    Router::new()
        .route("/relay/desktop/{id}/link", post(offer))
        .route("/relay/account/desktop-link", post(claim))
        .layer(DefaultBodyLimit::max(1024))
}
