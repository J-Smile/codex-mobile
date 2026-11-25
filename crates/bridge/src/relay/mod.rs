mod approval;
mod link;
mod tunnel;

use crate::{
    auth::random_token,
    http::Bridge,
    paths::{read_json_optional, write_private},
    BridgeError, Result,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use bytes::Bytes;
use link::{DesktopEvent, DesktopLink};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::{
    sync::{broadcast, mpsc, watch, Mutex, Notify},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;
use url::Url;

const NOISE: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2b";
const PROLOGUE: &[u8] = b"codex-mobile-relay/v1";
const LINK_LIFETIME: i64 = 5 * 60 * 1000;

#[derive(Clone, Deserialize, Serialize)]
struct Settings {
    url: String,
    name: String,
    token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    trust_scope: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct Identity {
    private: String,
    public: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Phone {
    id: String,
    name: String,
    paired_at: i64,
}

#[derive(Default)]
struct Trust {
    phones: BTreeMap<String, Phone>,
    ticket: Option<(String, i64)>,
    approval: Option<approval::Invite>,
    approver: Option<approval::Approver>,
    direct_pairing: Option<(String, i64)>,
}

pub struct RemoteAccess {
    directory: PathBuf,
    settings: watch::Sender<Option<Settings>>,
    phase: watch::Sender<&'static str>,
    trust: Mutex<Trust>,
    revoked: broadcast::Sender<String>,
    link_requested: Notify,
}

fn origin(url: &str) -> Result<String> {
    let url =
        Url::parse(url).map_err(|_| BridgeError::invalid("请输入手机网页的完整 HTTPS 地址。"))?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if (url.scheme() != "https" && !(local && url.scheme() == "http"))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(BridgeError::invalid(
            "云端地址只能填写 HTTPS 网站的根地址。",
        ));
    }
    Ok(url.origin().ascii_serialization())
}

fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 48 && !name.chars().any(char::is_control)
}

impl Settings {
    fn valid(&self) -> Result<()> {
        origin(&self.url)?;
        if !valid_name(&self.name) {
            return Err(BridgeError::invalid("设备名称需是 1 至 48 个字符。"));
        }
        if !(32..=256).contains(&self.token.len())
            || !self
                .token
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
        {
            return Err(BridgeError::invalid("请输入账号的电脑接入密钥。"));
        }
        if self.trust_scope.as_ref().is_some_and(|scope| {
            scope.len() != 64
                || !scope
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        }) {
            return Err(BridgeError::invalid("电脑的云连接配置无效。"));
        }
        Ok(())
    }

    fn http(&self, path: &str) -> String {
        format!("{}{}", self.url, path)
    }

    fn account_scope(&self) -> String {
        crate::digest(format!("{}\n{}", self.url, self.token))
    }
}

fn load_phones(directory: &Path, settings: Option<&Settings>) -> Result<BTreeMap<String, Phone>> {
    read_json_optional(&phones_path(directory, settings))?
        .map(serde_json::from_value)
        .transpose()
        .map(|phones| phones.unwrap_or_default())
        .map_err(Into::into)
}

fn phones_path(directory: &Path, settings: Option<&Settings>) -> PathBuf {
    settings
        .and_then(|settings| settings.trust_scope.as_ref())
        .map(|scope| directory.join(format!("relay-phones-{scope}.json")))
        .unwrap_or_else(|| directory.join("relay-phones.json"))
}

impl Identity {
    fn read(directory: &Path) -> Result<Option<Self>> {
        read_json_optional(&directory.join("relay-identity.json"))?
            .map(serde_json::from_value)
            .transpose()
            .map_err(Into::into)
    }

    fn open(directory: &Path) -> Result<Self> {
        if let Some(identity) = Self::read(directory)? {
            return Ok(identity);
        }
        let key = snow::Builder::new(NOISE.parse().map_err(link::protocol_error)?)
            .generate_keypair()
            .map_err(link::protocol_error)?;
        let identity = Self {
            private: hex::encode(key.private),
            public: hex::encode(key.public),
        };
        write_private(
            &directory.join("relay-identity.json"),
            &serde_json::to_vec(&identity)?,
        )?;
        Ok(identity)
    }

    fn id(&self) -> Result<String> {
        Ok(crate::digest(
            hex::decode(&self.public).map_err(link::protocol_error)?,
        ))
    }
}

impl RemoteAccess {
    pub fn open(directory: PathBuf) -> Result<Self> {
        let settings: Option<Settings> =
            read_json_optional(&directory.join("relay-settings.json"))?
                .filter(|value| !value.is_null())
                .map(serde_json::from_value)
                .transpose()?;
        if let Some(settings) = &settings {
            settings.valid()?;
        }
        let phones = load_phones(&directory, settings.as_ref())?;
        let approver = read_json_optional(&directory.join("relay-approver.json"))?
            .map(serde_json::from_value)
            .transpose()?;
        Ok(Self {
            directory,
            settings: watch::channel(settings).0,
            phase: watch::channel("stopped").0,
            trust: Mutex::new(Trust {
                phones,
                ticket: None,
                approval: None,
                approver,
                direct_pairing: None,
            }),
            revoked: broadcast::channel(16).0,
            link_requested: Notify::new(),
        })
    }

    pub async fn status(&self) -> Value {
        let settings = self.settings.borrow().clone();
        let state = if settings.is_some() {
            *self.phase.borrow()
        } else {
            "disabled"
        };
        let trust = self.trust.lock().await;
        let phones: Vec<_> = trust.phones.values().cloned().collect();
        json!({
            "state": state,
            "url": settings.as_ref().map(|settings| settings.url.as_str()).unwrap_or(""),
            "name": settings.as_ref().map(|settings| settings.name.as_str()).unwrap_or(""),
            "hasToken": settings.as_ref().is_some_and(|settings| !settings.token.starts_with("cm_device_")),
            "linkExpiresAt": trust.direct_pairing.as_ref().map(|(_, expiration)| *expiration).filter(|expiration| *expiration > crate::now_ms()),
            "hasInvite": trust.ticket.as_ref().is_some_and(|(_, expires)| *expires > crate::now_ms()),
            "approval": trust.approval.as_ref().filter(|invite| invite.is_current()).map(|invite| invite.status()),
            "canApprove": settings.as_ref().is_some_and(|settings| {
                trust.approver.as_ref().is_some_and(|approver| approver.matches(settings))
            }),
            "phones": phones,
        })
    }

    pub async fn configure(&self, url: &str, name: &str, token: &str) -> Result<()> {
        let mut trust = self.trust.lock().await;
        let previous = self.settings.borrow().clone();
        let settings = if url.trim().is_empty() {
            None
        } else {
            let url = origin(url.trim())?;
            let mut settings = Settings {
                url: url.clone(),
                name: if name.trim().is_empty() {
                    "Codex 电脑".into()
                } else {
                    name.trim().into()
                },
                token: if token.trim().is_empty() {
                    previous
                        .as_ref()
                        .filter(|settings| settings.url == url)
                        .map(|settings| settings.token.clone())
                        .unwrap_or_else(|| format!("cm_device_{}", random_token()))
                } else {
                    token.trim().into()
                },
                trust_scope: None,
            };
            settings.valid()?;
            if let Some(previous) = previous
                .as_ref()
                .filter(|previous| previous.url == settings.url && previous.token == settings.token)
            {
                settings.trust_scope = previous.trust_scope.clone();
            } else {
                settings.trust_scope = Some(settings.account_scope());
            }
            Some(settings)
        };
        let phones = load_phones(&self.directory, settings.as_ref())?;
        if let Some(previous) = previous.as_ref().filter(|previous| {
            previous.trust_scope.is_none()
                && settings
                    .as_ref()
                    .is_none_or(|settings| settings.trust_scope.is_some())
        }) {
            let path = self
                .directory
                .join(format!("relay-phones-{}.json", previous.account_scope()));
            write_private(&path, &serde_json::to_vec(&trust.phones)?)?;
        }
        write_private(
            &self.directory.join("relay-settings.json"),
            &serde_json::to_vec(&settings)?,
        )?;
        trust.ticket = None;
        trust.approval = None;
        trust.direct_pairing = None;
        trust.phones = phones;
        self.phase.send_replace("stopped");
        self.settings.send_replace(settings);
        Ok(())
    }

    pub async fn link_from_phone(&self) -> Result<Value> {
        let mut trust = self.trust.lock().await;
        let settings =
            self.settings.borrow().clone().ok_or_else(|| {
                BridgeError::invalid("请先在云连接设置中填写自己的手机网页地址。")
            })?;
        let identity = Identity::open(&self.directory)?;
        let id = identity.id()?;
        let ticket = random_token();
        let now = crate::now_ms();
        let (expires_at, claim) =
            if settings.token.starts_with("cm_device_") && *self.phase.borrow() != "online" {
                let claim = random_token();
                let response = reqwest::Client::builder()
                    .timeout(link::CONNECT_TIMEOUT)
                    .redirect(reqwest::redirect::Policy::none())
                    .build()?
                    .post(settings.http(&format!("/relay/desktop/{id}/link")))
                    .bearer_auth(&settings.token)
                    .json(&json!({
                        "code": URL_SAFE_NO_PAD.encode(Sha256::digest(claim.as_bytes())),
                        "name": settings.name,
                    }))
                    .send()
                    .await?;
                match response.status().as_u16() {
                    200 => {}
                    409 => {
                        return Err(BridgeError::unavailable(
                            "这台电脑已加入账号，请等云连接上线后重新出码。",
                        ))
                    }
                    404 => return Err(BridgeError::unavailable("云中转需要更新后才能扫码添加。")),
                    429 => return Err(BridgeError::unavailable("当前扫码请求较多，请稍后重试。")),
                    _ => {
                        return Err(BridgeError::unavailable(
                            "云端暂时无法生成二维码，请稍后重试。",
                        ))
                    }
                }
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct LinkLifetime {
                    expires_in: i64,
                }
                let lifetime = response.json::<LinkLifetime>().await?;
                (now + lifetime.expires_in.clamp(1, 180) * 1000, Some(claim))
            } else {
                if *self.phase.borrow() != "online" {
                    return Err(BridgeError::unavailable("正在连接云中转，请稍后重试。"));
                }
                (now + 180_000, None)
            };
        // 桌面登记和 Noise 配对使用不同的凭证。配对凭证只由桌面和扫码设备持有。
        trust.direct_pairing = Some((ticket.clone(), expires_at));
        if claim.is_some() {
            self.link_requested.notify_one();
        }
        let payload = json!({
            "version": 3, "publicKey": identity.public, "ticket": ticket,
            "expiresAt": expires_at, "name": settings.name, "claim": claim,
        });
        let link = format!(
            "{}/#pair={}",
            settings.url,
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload)?)
        );
        Ok(json!({ "link": link, "expiresAt": expires_at }))
    }

    pub async fn pairing(&self) -> Result<Value> {
        let mut trust = self.trust.lock().await;
        let settings = self
            .settings
            .borrow()
            .clone()
            .filter(|_| *self.phase.borrow() == "online")
            .ok_or_else(|| BridgeError::unavailable("请先连接云中转。"))?;
        let identity = Identity::read(&self.directory)?
            .ok_or_else(|| BridgeError::unavailable("桌面身份尚未就绪。"))?;
        let ticket = random_token();
        let expires_at = crate::now_ms() + LINK_LIFETIME;
        let payload = json!({
            "version": 1, "publicKey": identity.public, "ticket": ticket,
            "expiresAt": expires_at, "name": settings.name
        });
        let link = format!(
            "{}/#pair={}",
            settings.url,
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload)?)
        );
        trust.ticket = Some((ticket, expires_at));
        Ok(json!({"link": link, "expiresAt": expires_at}))
    }

    fn save_phones(&self, phones: &BTreeMap<String, Phone>) -> Result<()> {
        let path = phones_path(&self.directory, self.settings.borrow().as_ref());
        write_private(&path, &serde_json::to_vec(phones)?)
    }

    fn remember_phone(&self, trust: &mut Trust, phone: Phone) -> Result<()> {
        if !trust.phones.contains_key(&phone.id) {
            let mut phones = trust.phones.clone();
            phones.insert(phone.id.clone(), phone);
            self.save_phones(&phones)?;
            trust.phones = phones;
        }
        Ok(())
    }

    async fn authorize(&self, id: &str, ticket: &str, name: &str) -> Result<bool> {
        let mut trust = self.trust.lock().await;
        if ticket.is_empty() && trust.phones.contains_key(id) {
            return Ok(true);
        }
        let valid_ticket = |(expected, expires): &(String, i64)| {
            *expires > crate::now_ms() && bool::from(ticket.as_bytes().ct_eq(expected.as_bytes()))
        };
        let direct = trust.direct_pairing.as_ref().is_some_and(valid_ticket);
        if !direct && !trust.ticket.as_ref().is_some_and(valid_ticket) {
            return Ok(false);
        }
        self.remember_phone(&mut trust, Phone::new(id.into(), name))?;
        if direct {
            trust.direct_pairing = None;
        } else {
            trust.ticket = None;
        }
        Ok(true)
    }

    pub async fn revoke(&self, phone: &str) -> Result<()> {
        let mut trust = self.trust.lock().await;
        let mut phones = trust.phones.clone();
        if phones.remove(phone).is_some() {
            self.save_phones(&phones)?;
            trust.phones = phones;
            let _ = self.revoked.send(phone.into());
        }
        Ok(())
    }

    pub(crate) async fn run(self: Arc<Self>, app: Arc<Bridge>, shutdown: CancellationToken) {
        let mut changes = self.settings.subscribe();
        loop {
            let settings = changes.borrow_and_update().clone();
            if let Some(settings) = settings {
                self.phase.send_replace("connecting");
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    _ = changes.changed() => continue,
                    result = self.control(&app, &settings) => {
                        self.trust.lock().await.approval = None;
                        let unlinked = settings.token.starts_with("cm_device_") && result.is_err_and(|error| error.status == 403);
                        self.phase.send_replace(if unlinked { "unlinked" } else { "offline" });
                        let linking = self.trust.lock().await.direct_pairing.as_ref()
                            .is_some_and(|(_, expiration)| *expiration > crate::now_ms());
                        let retry = async {
                            if !unlinked {
                                tokio::time::sleep(Duration::from_secs(3)).await;
                            } else if linking {
                                tokio::time::sleep(Duration::from_secs(1)).await;
                            } else {
                                self.link_requested.notified().await;
                            }
                        };
                        tokio::select! {
                            _ = shutdown.cancelled() => break,
                            _ = changes.changed() => continue,
                            _ = retry => {}
                        }
                    }
                }
            } else {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = changes.changed() => {}
                }
            }
        }
        let mut trust = self.trust.lock().await;
        trust.ticket = None;
        trust.approval = None;
        trust.direct_pairing = None;
        self.phase.send_replace("stopped");
    }

    async fn control(self: &Arc<Self>, app: &Arc<Bridge>, settings: &Settings) -> Result<()> {
        let identity = Arc::new(Identity::open(&self.directory)?);
        let id = identity.id()?;
        let (mut link, sender) = DesktopLink::connect(
            settings.http(&format!("/relay/desktop/{id}")),
            &settings.token,
            &settings.name,
        )
        .await?;
        if !matches!(link.next().await?, DesktopEvent::Ready) {
            return Err(link::protocol_error("missing registration event"));
        }
        self.phase.send_replace("online");
        let mut peers = JoinSet::<String>::new();
        let mut sessions = HashMap::<String, mpsc::Sender<Bytes>>::new();
        loop {
            let event = tokio::select! {
                result = peers.join_next(), if !peers.is_empty() => {
                    if let Some(Ok(session)) = result { sessions.remove(&session); }
                    continue;
                }
                event = link.next() => event?,
            };
            match event {
                DesktopEvent::Ready => {
                    return Err(link::protocol_error("duplicate registration event"))
                }
                DesktopEvent::Open(session) => {
                    let (tx, rx) = mpsc::channel(8);
                    sessions.insert(session.clone(), tx);
                    let app = app.clone();
                    let access = self.clone();
                    let identity = identity.clone();
                    let sender = sender.clone();
                    let origin = settings.url.clone();
                    peers.spawn(async move {
                        let _ =
                            tunnel::serve(app, &access, &identity, &sender, &session, &origin, rx)
                                .await;
                        sender.close(&session).await;
                        session
                    });
                }
                DesktopEvent::Batch { session, bytes } => {
                    if sessions
                        .get(&session)
                        .is_some_and(|tx| tx.try_send(bytes).is_err())
                    {
                        sessions.remove(&session);
                    }
                }
                DesktopEvent::Close(session) => {
                    sessions.remove(&session);
                }
                DesktopEvent::Approval { request, proof } => {
                    self.receive_approval(&identity, request, &proof).await;
                }
            }
        }
    }
}

impl Phone {
    fn new(id: String, name: &str) -> Self {
        let name = if valid_name(name) {
            name.trim().into()
        } else {
            format!("手机 {}", &id[..8])
        };
        Self {
            id,
            name,
            paired_at: crate::now_ms(),
        }
    }
}
