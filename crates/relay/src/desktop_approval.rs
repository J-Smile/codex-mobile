use super::{
    database_error, digest, random_key, session_key, valid_id, valid_key, Desktop, DesktopEvent,
    DesktopKey, Relay,
};
use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::{sync::oneshot, time::Instant};
use uuid::Uuid;

const LIFETIME: Duration = Duration::from_secs(180);
const APPROVER_HEADER: &str = "x-codex-desktop-approver";

#[derive(Clone, Default)]
pub(crate) struct Approvals(Arc<Mutex<HashMap<String, Invite>>>);

struct Invite {
    owner: DesktopKey,
    desktop: Weak<Desktop>,
    expires: Instant,
    pending: Option<(Uuid, oneshot::Sender<bool>)>,
}

#[derive(Deserialize)]
struct Invitation {
    code: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Enrollment {
    login_key: String,
    approver_key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    code: String,
    account_id: String,
    proof: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Decision {
    code: String,
    request_id: Uuid,
    approved: bool,
}

async fn enable(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(enrollment): Json<Enrollment>,
) -> Result<StatusCode, StatusCode> {
    let owner = relay.desktop_key(&headers, id)?;
    relay.desktop(&owner)?;
    if !enrollment
        .login_key
        .strip_prefix("cm_login_")
        .is_some_and(valid_key)
        || !valid_key(&enrollment.approver_key)
        || !relay
            .accounts
            .enable_desktop_approval(
                &owner.0,
                &owner.1,
                &enrollment.login_key,
                digest(&enrollment.approver_key),
            )
            .map_err(database_error)?
    {
        return Err(StatusCode::FORBIDDEN);
    }
    relay
        .approvals
        .0
        .lock()
        .unwrap()
        .retain(|_, invite| invite.owner != owner);
    Ok(StatusCode::NO_CONTENT)
}

fn approver(relay: &Relay, headers: &HeaderMap, id: String) -> Result<DesktopKey, StatusCode> {
    let owner = relay.desktop_key(headers, id)?;
    let key = headers
        .get(APPROVER_HEADER)
        .and_then(|key| key.to_str().ok())
        .filter(|key| valid_key(key))
        .ok_or(StatusCode::FORBIDDEN)?;
    if !relay
        .accounts
        .desktop_approver(&owner.0, &owner.1, digest(key))
        .map_err(database_error)?
    {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(owner)
}

async fn register(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(invite): Json<Invitation>,
) -> Result<Response, StatusCode> {
    let owner = approver(&relay, &headers, id)?;
    if !valid_id(&invite.code) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let desktop = relay.desktop(&owner)?;
    let now = Instant::now();
    let mut invites = relay.approvals.0.lock().unwrap();
    invites.retain(|_, entry| entry.expires > now && entry.owner != owner);
    if invites.contains_key(&invite.code) {
        return Err(StatusCode::CONFLICT);
    }
    let account_id = owner.0.clone();
    invites.insert(
        invite.code,
        Invite {
            owner,
            desktop: Arc::downgrade(&desktop),
            expires: now + LIFETIME,
            pending: None,
        },
    );
    Ok((
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(serde_json::json!({ "accountId": account_id, "expiresIn": LIFETIME.as_secs() })),
    )
        .into_response())
}

async fn request(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(input): Json<Request>,
) -> Result<Response, StatusCode> {
    if !relay.mobile_allowed(&headers) {
        return Err(StatusCode::FORBIDDEN);
    }
    if !valid_id(&input.code)
        || !(128..=512).contains(&input.proof.len())
        || !input
            .proof
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let previous = session_key(&headers);
    let signed_in_account = match previous {
        Some(key) => relay
            .accounts
            .session(key)
            .map_err(database_error)?
            .map(|(account, _)| account),
        None => None,
    };
    let request = Uuid::new_v4();
    let (owner, desktop, expires, decision) = {
        let mut invites = relay.approvals.0.lock().unwrap();
        let invite = invites
            .get_mut(&input.code)
            .filter(|invite| invite.expires > Instant::now())
            .ok_or(StatusCode::GONE)?;
        if input.account_id != invite.owner.0
            || signed_in_account.is_some_and(|account| account.id != invite.owner.0)
        {
            return Err(StatusCode::FORBIDDEN);
        }
        if invite.pending.is_some() {
            return Err(StatusCode::CONFLICT);
        }
        let desktop = invite
            .desktop
            .upgrade()
            .filter(|desktop| !desktop.closed.is_cancelled())
            .ok_or(StatusCode::NOT_FOUND)?;
        let (resolve, decision) = oneshot::channel();
        desktop
            .events
            .try_send(DesktopEvent::Approval {
                request,
                proof: input.proof,
            })
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        invite.pending = Some((request, resolve));
        (invite.owner.clone(), desktop, invite.expires, decision)
    };
    let approved = tokio::select! {
        biased;
        _ = desktop.closed.cancelled() => return Err(StatusCode::NOT_FOUND),
        _ = tokio::time::sleep_until(expires) => return Err(StatusCode::GONE),
        result = decision => result.map_err(|_| StatusCode::GONE)?,
    };
    if !approved {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let key = random_key();
    let account = relay
        .accounts
        .login_with_desktop(&owner.0, &owner.1, digest(&key), previous)
        .map_err(database_error)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    relay.signed_in(account, &key, previous)
}

async fn resolve(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(decision): Json<Decision>,
) -> Result<StatusCode, StatusCode> {
    let owner = approver(&relay, &headers, id)?;
    if !valid_id(&decision.code) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut invites = relay.approvals.0.lock().unwrap();
    let ready = invites.get(&decision.code).is_some_and(|entry| {
        entry.owner == owner
            && entry.expires > Instant::now()
            && entry
                .pending
                .as_ref()
                .is_some_and(|(request, _)| *request == decision.request_id)
    });
    if !ready {
        return Err(StatusCode::GONE);
    }
    let (_, response) = invites
        .remove(&decision.code)
        .and_then(|invite| invite.pending)
        .ok_or(StatusCode::GONE)?;
    response
        .send(decision.approved)
        .map_err(|_| StatusCode::GONE)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn routes() -> Router<Relay> {
    Router::new()
        .route("/relay/desktop/{id}/approval", post(register))
        .route("/relay/desktop/{id}/approval/enable", post(enable))
        .route("/relay/desktop/{id}/approval/decision", post(resolve))
        .route("/relay/account/desktop-approval", post(request))
        .layer(DefaultBodyLimit::max(1024))
}
