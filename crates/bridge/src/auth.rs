use crate::{
    paths::{read_json_optional, write_private},
    BridgeError, Result,
};
use axum::http::HeaderMap;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    sync::Arc,
};
use subtle::ConstantTimeEq;

pub const SESSION_AGE: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Clone)]
pub(crate) struct RelayPhone(Arc<str>);

impl RelayPhone {
    const PREFIX: &str = "relay:";

    pub(crate) fn new(fingerprint: &str) -> Self {
        Self(format!("{}{fingerprint}", Self::PREFIX).into())
    }

    pub(crate) fn owner(&self) -> &str {
        &self.0
    }

    pub(crate) fn owns_subscription(owner: &str) -> bool {
        owner.starts_with(Self::PREFIX)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedSessions {
    key_fingerprint: String,
    sessions: Vec<(String, i64)>,
}

pub struct Auth {
    key: String,
    directory: PathBuf,
    sessions: HashMap<String, i64>,
}

pub fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

impl Auth {
    pub fn open(directory: PathBuf) -> Result<Self> {
        let file = directory.join("access-key");
        let key = if file.exists() {
            std::fs::read_to_string(&file)?.trim().to_string()
        } else {
            let key = random_token();
            write_private(&file, key.as_bytes())?;
            key
        };
        let saved = read_json_optional(&directory.join("sessions.json"))?
            .map(serde_json::from_value::<SavedSessions>)
            .transpose()?;
        let sessions = saved
            .filter(|s| s.key_fingerprint == crate::digest(&key))
            .map(|s| {
                s.sessions
                    .into_iter()
                    .filter(|(_, expires)| *expires > crate::now_ms())
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            key,
            directory,
            sessions,
        })
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn save(&self) -> Result<()> {
        let saved = SavedSessions {
            key_fingerprint: crate::digest(&self.key),
            sessions: self
                .sessions
                .iter()
                .filter(|(_, expires)| **expires > crate::now_ms())
                .map(|(token, expires)| (token.clone(), *expires))
                .collect(),
        };
        write_private(
            &self.directory.join("sessions.json"),
            &serde_json::to_vec(&saved)?,
        )
    }
    pub fn login(&mut self, candidate: &str) -> Result<String> {
        if !bool::from(self.key.as_bytes().ct_eq(candidate.as_bytes())) {
            return Err(BridgeError::new(401, "连接密钥不正确。"));
        }
        let token = random_token();
        self.sessions
            .insert(crate::digest(&token), crate::now_ms() + SESSION_AGE);
        self.save()?;
        Ok(token)
    }
    pub fn session(&self, headers: &HeaderMap) -> Result<(String, i64)> {
        let token = header(headers, "cookie")
            .split(';')
            .map(str::trim)
            .find_map(|c| c.strip_prefix("cm_session="))
            .unwrap_or("");
        let owner = crate::digest(token);
        self.sessions
            .get(&owner)
            .filter(|e| **e > crate::now_ms())
            .map(|e| (owner, *e))
            .ok_or_else(|| BridgeError::new(401, "请先连接你的电脑。"))
    }
    pub fn logout(&mut self, owner: &str) -> Result<()> {
        self.sessions.remove(owner);
        self.save()
    }
    pub fn rotate(&mut self, owner: &str) -> Result<String> {
        let expires = self
            .sessions
            .get(owner)
            .copied()
            .ok_or_else(|| BridgeError::new(401, "请重新连接。"))?;
        self.key = random_token();
        write_private(&self.directory.join("access-key"), self.key.as_bytes())?;
        self.sessions.clear();
        self.sessions.insert(owner.into(), expires);
        self.save()?;
        Ok(self.key.clone())
    }
    pub fn rotate_local(&mut self) -> Result<String> {
        self.key = random_token();
        write_private(&self.directory.join("access-key"), self.key.as_bytes())?;
        self.sessions.clear();
        self.save()?;
        Ok(self.key.clone())
    }
}

pub fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

pub fn private_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(|v4| private_address(IpAddr::V4(v4)))
            .unwrap_or(ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()),
    }
}

pub fn pairing_request(headers: &HeaderMap, remote: IpAddr) -> bool {
    if headers.keys().any(|n| {
        n == "forwarded"
            || n.as_str().starts_with("x-forwarded-")
            || n == "x-real-ip"
            || n == "cf-connecting-ip"
    }) {
        return false;
    }
    if !private_address(remote) {
        return false;
    }
    let Ok(url) = url::Url::parse(&format!("http://{}", header(headers, "host"))) else {
        return false;
    };
    let host = url.host_str().unwrap_or("").trim_matches(['[', ']']);
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(private_address)
}

pub fn check_origin(headers: &HeaderMap) -> Result<()> {
    if !headers.contains_key("origin") {
        return Ok(());
    }
    let url = url::Url::parse(header(headers, "origin"))
        .map_err(|_| BridgeError::new(403, "请求来源不匹配。"))?;
    if url[url::Position::BeforeHost..url::Position::AfterPort] != *header(headers, "host") {
        return Err(BridgeError::new(403, "请求来源不匹配。"));
    }
    Ok(())
}

pub fn network_urls(port: u16) -> Vec<serde_json::Value> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            let IpAddr::V4(ip) = entry.ip() else {
                return None;
            };
            if ip.is_loopback()
                || ip.is_link_local()
                || ip == Ipv4Addr::UNSPECIFIED
                || ["Npcap", "Loopback", "vEthernet", "VirtualBox", "VMware"]
                    .iter()
                    .any(|n| entry.name.contains(n))
            {
                return None;
            }
            Some(serde_json::json!({"name":entry.name,"url":format!("http://{ip}:{port}")}))
        })
        .collect()
}
