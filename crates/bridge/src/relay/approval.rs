use super::{
    link::{protocol_error, DesktopSender},
    Identity, Phone, RemoteAccess, Settings, NOISE,
};
use crate::{auth::random_token, digest, now_ms, paths::write_private, BridgeError, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const PROLOGUE: &[u8] = b"codex-mobile-desktop-approval/v1";

pub(super) struct Invite {
    sender: DesktopSender,
    approver_key: String,
    ticket: String,
    expires_at: i64,
    pending: Option<Pending>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Approver {
    origin: String,
    desktop_token: String,
    key: String,
}

impl Approver {
    pub(super) fn matches(&self, settings: &Settings) -> bool {
        self.origin == settings.url && self.desktop_token == digest(&settings.token)
    }
}

struct Pending {
    request: String,
    phone: Phone,
    code: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registered {
    account_id: String,
    expires_in: i64,
}

#[derive(Deserialize)]
struct Proof {
    ticket: String,
    name: String,
}

impl Invite {
    pub(super) fn is_current(&self) -> bool {
        self.expires_at > now_ms()
    }

    pub(super) fn status(&self) -> Value {
        json!({
            "expiresAt": self.expires_at,
            "pending": self.pending.as_ref().map(|pending| json!({
                "id": pending.request, "name": pending.phone.name, "code": pending.code,
            })),
        })
    }
}

fn read_proof(identity: &Identity, ciphertext: &[u8]) -> Result<(String, Proof)> {
    let private = hex::decode(&identity.private).map_err(protocol_error)?;
    let mut handshake = snow::Builder::new(NOISE.parse().map_err(protocol_error)?)
        .prologue(PROLOGUE)
        .map_err(protocol_error)?
        .local_private_key(&private)
        .map_err(protocol_error)?
        .build_responder()
        .map_err(protocol_error)?;
    let mut decrypted = [0; 384];
    let size = handshake
        .read_message(ciphertext, &mut decrypted)
        .map_err(protocol_error)?;
    let phone_key = handshake
        .get_remote_static()
        .ok_or_else(|| protocol_error("phone key"))?;
    Ok((
        digest(phone_key),
        serde_json::from_slice(&decrypted[..size])?,
    ))
}

impl RemoteAccess {
    pub async fn start_approval(&self, login_key: Option<&str>) -> Result<Value> {
        let settings = self
            .settings
            .borrow()
            .clone()
            .filter(|_| *self.phase.borrow() == "online")
            .ok_or_else(|| BridgeError::unavailable("请先连接云中转。"))?;
        let identity = Identity::read(&self.directory)?
            .ok_or_else(|| BridgeError::unavailable("桌面身份尚未就绪。"))?;
        let id = identity.id()?;
        let sender = DesktopSender::new(
            settings.http(&format!("/relay/desktop/{id}")),
            &settings.token,
        )?;
        let approver_key = if let Some(login_key) = login_key {
            let approver = Approver {
                origin: settings.url.clone(),
                desktop_token: digest(&settings.token),
                key: random_token(),
            };
            sender
                .approval(
                    "/enable",
                    "",
                    json!({
                        "loginKey": login_key.trim(), "approverKey": approver.key,
                    }),
                )
                .await?;
            write_private(
                &self.directory.join("relay-approver.json"),
                &serde_json::to_vec(&approver)?,
            )?;
            let key = approver.key.clone();
            self.trust.lock().await.approver = Some(approver);
            key
        } else {
            self.trust
                .lock()
                .await
                .approver
                .as_ref()
                .filter(|approver| approver.matches(&settings))
                .map(|approver| approver.key.clone())
                .ok_or_else(|| BridgeError::invalid("请先使用网页登录密钥验证这台电脑。"))?
        };
        let ticket = random_token();
        let started_at = now_ms();
        let Registered {
            account_id,
            expires_in,
        } = sender
            .approval("", &approver_key, json!({"code": digest(&ticket)}))
            .await?
            .json()
            .await?;
        let expires_at = started_at + expires_in.clamp(1, 180) * 1000;
        let payload = json!({
            "version": 2, "publicKey": identity.public, "ticket": ticket,
            "expiresAt": expires_at, "name": settings.name, "accountId": account_id,
        });
        let link = format!(
            "{}/#pair={}",
            settings.url,
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload)?)
        );
        self.trust.lock().await.approval = Some(Invite {
            sender,
            approver_key,
            ticket,
            expires_at,
            pending: None,
        });
        Ok(json!({ "link": link, "expiresAt": expires_at }))
    }

    pub(super) async fn receive_approval(
        &self,
        identity: &Identity,
        request: String,
        ciphertext: &[u8],
    ) {
        let mut trust = self.trust.lock().await;
        let Some(invite) = trust
            .approval
            .as_mut()
            .filter(|invite| invite.is_current() && invite.pending.is_none())
        else {
            return;
        };
        // 仅从通过 Noise 校验的消息读取手机身份，授权码原文只在二维码和密文内。
        if let Ok((id, proof)) = read_proof(identity, ciphertext) {
            if bool::from(proof.ticket.as_bytes().ct_eq(invite.ticket.as_bytes())) {
                let fingerprint = Sha256::digest(ciphertext);
                let code = u32::from_be_bytes(fingerprint[..4].try_into().unwrap()) % 1_000_000;
                invite.pending = Some(Pending {
                    request,
                    phone: Phone::new(id, &proof.name),
                    code: format!("{code:06}"),
                });
                return;
            }
        }
        let sender = invite.sender.clone();
        let approver_key = invite.approver_key.clone();
        let code = digest(&invite.ticket);
        drop(trust);
        tokio::spawn(async move {
            let _ = sender
                .approval(
                    "/decision",
                    &approver_key,
                    json!({ "code": code, "requestId": request, "approved": false }),
                )
                .await;
        });
    }

    pub async fn decide_approval(&self, request: &str, approved: bool) -> Result<()> {
        let mut trust = self.trust.lock().await;
        let invite = trust
            .approval
            .as_ref()
            .filter(|invite| invite.is_current())
            .ok_or_else(|| BridgeError::invalid("授权码已失效，请重新授权。"))?;
        let pending = invite
            .pending
            .as_ref()
            .filter(|pending| pending.request == request)
            .ok_or_else(|| BridgeError::invalid("手机的授权请求已变化，请核对后重试。"))?;
        let (sender, approver_key, code) = (
            invite.sender.clone(),
            invite.approver_key.clone(),
            digest(&invite.ticket),
        );
        let new_phone = (approved && !trust.phones.contains_key(&pending.phone.id))
            .then(|| pending.phone.clone());
        if let Some(phone) = &new_phone {
            self.remember_phone(&mut trust, phone.clone())?;
        }
        trust.approval = None;
        drop(trust);
        let result = sender
            .approval(
                "/decision",
                &approver_key,
                json!({ "code": code, "requestId": request, "approved": approved }),
            )
            .await;
        if result
            .as_ref()
            .is_err_and(|error| matches!(error.status, 403 | 410))
        {
            if let Some(phone) = new_phone {
                self.revoke(&phone.id).await?;
            }
        }
        result.map(|_| ())
    }
}
