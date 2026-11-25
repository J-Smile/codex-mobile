mod support;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use bytes::{Buf, Bytes, BytesMut};
use codex_mobile_bridge::{config::ServiceConfig, host::Host, relay::RemoteAccess};
use codex_mobile_relay::{AccountKeys, Accounts};
use reqwest::{Client, Response};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use support::{Fixture, MockDesktop};
use tokio::{io::AsyncWriteExt, net::TcpListener, task::JoinHandle};

const NOISE: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2b";
const PROLOGUE: &[u8] = b"codex-mobile-relay/v1";
const SESSION_HEADER: &str = "x-codex-relay-session";

#[tokio::test]
async fn cloud_pairing_requires_explicit_server_and_stays_disabled_after_clearing() {
    let fixture = Fixture::new();
    let directory = &fixture.paths.data_home;
    let remote = RemoteAccess::open(directory.clone()).unwrap();
    let error = remote.link_from_phone().await.unwrap_err();
    assert_eq!(error.status, 400);
    assert!(error.message.contains("云连接设置"));
    assert_eq!(remote.status().await["state"], "disabled");
    assert!(!directory.join("relay-settings.json").exists());
    assert!(!directory.join("relay-identity.json").exists());

    remote
        .configure("https://mobile.example.com", "", "")
        .await
        .unwrap();
    assert_eq!(remote.status().await["name"], "Codex 电脑");
    drop(remote);
    let remote = RemoteAccess::open(directory.clone()).unwrap();
    assert_eq!(remote.status().await["url"], "https://mobile.example.com");
    remote.configure("", "", "").await.unwrap();
    drop(remote);
    let remote = RemoteAccess::open(directory.clone()).unwrap();
    assert_eq!(remote.link_from_phone().await.unwrap_err().status, 400);
    assert_eq!(remote.status().await["state"], "disabled");
    assert_eq!(remote.status().await["url"], "");
    assert_eq!(
        std::fs::read_to_string(directory.join("relay-settings.json")).unwrap(),
        "null"
    );
    assert!(!directory.join("relay-identity.json").exists());
}

async fn relay_server(accounts: Accounts) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let router = codex_mobile_relay::router(origin.clone(), accounts);
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (origin, server)
}

#[tokio::test]
async fn cloud_pairing_uses_only_the_saved_server_and_device_name() {
    let fixture = Fixture::new();
    let accounts = Accounts::open(std::path::Path::new(":memory:")).unwrap();
    let (origin, server) = relay_server(accounts).await;
    let remote = RemoteAccess::open(fixture.paths.data_home.clone()).unwrap();
    remote.configure(&origin, "自定义电脑", "").await.unwrap();
    let invite = remote.link_from_phone().await.unwrap();
    let link = url::Url::parse(invite["link"].as_str().unwrap()).unwrap();
    assert_eq!(link.origin().ascii_serialization(), origin);
    let payload: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(link.fragment().unwrap().strip_prefix("pair=").unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(payload["name"], "自定义电脑");
    assert_eq!(payload["version"], 3);
    assert!(payload["claim"].is_string());
    assert!(invite["expiresAt"].as_i64().unwrap() > codex_mobile_bridge::now_ms());
    server.abort();
}

async fn relay() -> (String, JoinHandle<()>, AccountKeys) {
    let accounts = Accounts::open(std::path::Path::new(":memory:")).unwrap();
    let owner = accounts.create("测试账号").unwrap();
    let (origin, server) = relay_server(accounts).await;
    (origin, server, owner)
}

async fn login(client: &Client, origin: &str, key: &str) -> String {
    let response = client
        .post(format!("{origin}/relay/account"))
        .header("origin", origin)
        .json(&json!({"loginKey": key}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let cookie = response.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.contains("Secure; HttpOnly; SameSite=Strict; Path=/"));
    cookie.split_once(';').unwrap().0.into()
}

#[tokio::test]
async fn passkey_challenges_check_origin_account_and_replay() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let accounts = Accounts::open(std::path::Path::new(":memory:")).unwrap();
        let owner = accounts.create("Passkey owner").unwrap();
        let other = accounts.create("Other account").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = format!("http://localhost:{}", address.port());
        let router = codex_mobile_relay::router(origin.clone(), accounts);
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = Client::builder()
            .no_proxy()
            .resolve("localhost", address)
            .build()
            .unwrap();
        let root = format!("{origin}/relay/account/passkeys");
        let create_options = || {
            client
                .post(format!("{root}/create/options"))
                .header("origin", &origin)
        };
        assert_eq!(create_options().send().await.unwrap().status(), 401);
        let origin_rejected = client
            .post(format!("{root}/sign-in/options"))
            .header("origin", "https://other.invalid")
            .send()
            .await
            .unwrap();
        assert_eq!(origin_rejected.status(), 403);

        let session = login(&client, &origin, &owner.login_key).await;
        let different_session = login(&client, &origin, &other.login_key).await;
        let options = create_options()
            .header("cookie", &session)
            .send()
            .await
            .unwrap();
        assert_eq!(options.status(), 200);
        let cookie = options.headers()["set-cookie"].to_str().unwrap();
        assert!(cookie.contains("Secure; HttpOnly; SameSite=Strict"));
        let ceremony = cookie.split_once(';').unwrap().0.to_owned();
        let public_key = options.json::<Value>().await.unwrap()["publicKey"].clone();
        assert_eq!(public_key["rp"]["id"], "localhost");
        assert_eq!(
            public_key["authenticatorSelection"]["residentKey"],
            "required"
        );
        assert_eq!(
            public_key["authenticatorSelection"]["userVerification"],
            "required"
        );

        let invalid_proof = json!({
            "id":"AQ","rawId":"AQ","type":"public-key",
            "response":{"attestationObject":"AQ","clientDataJSON":"AQ"},
        });
        let finish = |session: &str, ceremony: &str| {
            client
                .post(format!("{root}/create"))
                .header("origin", &origin)
                .header("cookie", format!("{session}; {ceremony}"))
                .json(&invalid_proof)
        };
        assert_eq!(
            finish(&different_session, &ceremony)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(
            finish(&session, &ceremony).send().await.unwrap().status(),
            401
        );

        let options = create_options()
            .header("cookie", &session)
            .send()
            .await
            .unwrap();
        let cookie = options.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split_once(';')
            .unwrap()
            .0;
        assert_ne!(cookie, ceremony);
        assert_eq!(finish(&session, cookie).send().await.unwrap().status(), 400);
        assert_eq!(finish(&session, cookie).send().await.unwrap().status(), 401);

        let options = client
            .post(format!("{root}/sign-in/options"))
            .header("origin", &origin)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(options["publicKey"]["allowCredentials"], json!([]));
        assert_eq!(options["publicKey"]["userVerification"], "required");
        server.abort();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn desktop_qr_links_the_named_computer_once_to_the_signed_in_account() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let accounts = Accounts::open(std::path::Path::new(":memory:")).unwrap();
        let owner = accounts.create("扫码账号").unwrap();
        let other = accounts.create("隔离账号").unwrap();
        let (origin, server) = relay_server(accounts).await;
        let client = Client::new();
        let login_a = login(&client, &origin, &owner.login_key).await;
        let login_b = login(&client, &origin, &other.login_key).await;
        let id = "a".repeat(64);
        let other_id = "b".repeat(64);
        let token = format!("cm_device_{}", "c".repeat(43));
        let code = "d".repeat(43);
        let code_hash = URL_SAFE_NO_PAD.encode(Sha256::digest(code.as_bytes()));
        let offer = || {
            client
                .post(format!("{origin}/relay/desktop/{id}/link"))
                .bearer_auth(&token)
                .json(&json!({"code": code_hash, "name": "扫码电脑"}))
        };
        let claim = |desktop_id: &str| {
            client
                .post(format!("{origin}/relay/account/desktop-link"))
                .header("origin", &origin)
                .json(&json!({"code": code, "desktopId": desktop_id}))
        };
        assert_eq!(offer().send().await.unwrap().status(), 200);
        assert_eq!(claim(&id).send().await.unwrap().status(), 401);
        assert_eq!(
            claim(&other_id)
                .header("cookie", &login_a)
                .send()
                .await
                .unwrap()
                .status(),
            410
        );
        assert_eq!(
            claim(&id)
                .header("cookie", &login_a)
                .send()
                .await
                .unwrap()
                .status(),
            204
        );
        assert_eq!(
            claim(&id)
                .header("cookie", &login_b)
                .send()
                .await
                .unwrap()
                .status(),
            410
        );
        assert_eq!(offer().send().await.unwrap().status(), 409);
        let registered = register(&client, &origin, &id, &token, "扫码电脑").await;
        let wrong_desktop = client
            .get(format!("{origin}/relay/desktop/{other_id}"))
            .bearer_auth(&token)
            .header("x-codex-desktop-name", URL_SAFE_NO_PAD.encode("另一台电脑"));
        assert_eq!(wrong_desktop.send().await.unwrap().status(), 403);
        let old_credential = client
            .get(format!("{origin}/relay/desktop/{id}"))
            .bearer_auth(&owner.desktop_key)
            .header("x-codex-desktop-name", URL_SAFE_NO_PAD.encode("另一台电脑"));
        assert_eq!(old_credential.send().await.unwrap().status(), 403);
        let directory = |cookie: &str| {
            client
                .get(format!("{origin}/relay/account"))
                .header("cookie", cookie)
        };
        let a: Value = directory(&login_a)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let b: Value = directory(&login_b)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            a["desktops"],
            json!([{"id": id, "name": "扫码电脑", "online": true}])
        );
        assert_eq!(b["desktops"], json!([]));
        let foreign_mobile = client
            .post(format!("{origin}/relay/mobile/{id}/session"))
            .header("origin", &origin)
            .header("cookie", &login_b);
        assert_eq!(foreign_mobile.send().await.unwrap().status(), 404);
        drop(registered);
        server.abort();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn desktop_approval_issues_a_single_session_for_the_confirmed_account() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let accounts = Accounts::open(std::path::Path::new(":memory:")).unwrap();
        let owner = accounts.create("授权账号").unwrap();
        let other = accounts.create("其他账号").unwrap();
        let (origin, server) = relay_server(accounts).await;
        let client = Client::new();
        let desktop = "a".repeat(64);
        let mut events = register(&client, &origin, &desktop, &owner.desktop_key, "授权电脑").await;
        let base = format!("{origin}/relay/desktop/{desktop}/approval");
        let endpoint = format!("{origin}/relay/account/desktop-approval");
        let approver = "b".repeat(43);
        let code = "c".repeat(64);
        let enrollment = |login_key: &str| {
            client
                .post(format!("{base}/enable"))
                .bearer_auth(&owner.desktop_key)
                .json(&json!({ "loginKey": login_key, "approverKey": approver }))
        };
        let issue = |code: &str| {
            client
                .post(&base)
                .bearer_auth(&owner.desktop_key)
                .header("x-codex-desktop-approver", &approver)
                .json(&json!({ "code": code }))
        };
        assert_eq!(issue(&code).send().await.unwrap().status(), 403);
        assert_eq!(
            enrollment(&other.login_key).send().await.unwrap().status(),
            403
        );
        assert_eq!(
            enrollment(&owner.login_key).send().await.unwrap().status(),
            204
        );
        let registered: Value = issue(&code)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        let account = registered["accountId"].as_str().unwrap();
        let other_cookie = login(&client, &origin, &other.login_key).await;
        let ask = |code: &str| {
            client
                .post(&endpoint)
                .header("origin", &origin)
                .json(&json!({ "code": code, "accountId": account, "proof": "d".repeat(180) }))
        };
        assert_eq!(
            ask(&code)
                .header("cookie", other_cookie)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        let mut wrong_origin = ask(&code).build().unwrap();
        wrong_origin
            .headers_mut()
            .insert("origin", "https://elsewhere.invalid".parse().unwrap());
        assert_eq!(client.execute(wrong_origin).await.unwrap().status(), 403);
        let pending = tokio::spawn(ask(&code).send());
        let (event, request, _) = events.next().await.unwrap();
        assert_eq!(event, "approval");
        assert!(!pending.is_finished(), "桌面确认前不能返回登录会话");
        assert_eq!(ask(&code).send().await.unwrap().status(), 409);
        let decide = |code: &str, request: &str, token: &str, approved: bool| {
            client
                .post(format!("{base}/decision"))
                .bearer_auth(token)
                .header("x-codex-desktop-approver", &approver)
                .json(&json!({ "requestId": request, "code": code, "approved": approved }))
        };
        assert_eq!(
            decide(&code, &request, &other.desktop_key, true)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        assert_eq!(
            decide(&code, &request, &owner.desktop_key, true)
                .send()
                .await
                .unwrap()
                .status(),
            204
        );
        let response = pending.await.unwrap().unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["cache-control"], "private, no-store");
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(cookie.contains("Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000"));
        assert_eq!(response.json::<Value>().await.unwrap()["id"], account);
        assert_eq!(ask(&code).send().await.unwrap().status(), 410);

        let code = "e".repeat(64);
        assert_eq!(issue(&code).send().await.unwrap().status(), 200);
        let pending = tokio::spawn(ask(&code).send());
        let (_, request, _) = events.next().await.unwrap();
        assert_eq!(
            decide(&code, &request, &owner.desktop_key, false)
                .send()
                .await
                .unwrap()
                .status(),
            204
        );
        let rejected = pending.await.unwrap().unwrap();
        assert_eq!(rejected.status(), 401);
        assert!(!rejected.headers().contains_key("set-cookie"));
        server.abort();
    })
    .await
    .unwrap();
}

async fn register(client: &Client, origin: &str, id: &str, key: &str, name: &str) -> EventStream {
    let mut stream = EventStream::new(
        client
            .get(format!("{origin}/relay/desktop/{id}"))
            .bearer_auth(key)
            .header("x-codex-desktop-name", URL_SAFE_NO_PAD.encode(name))
            .send()
            .await
            .unwrap(),
    );
    assert_eq!(stream.next().await.unwrap().0, "ready");
    stream
}

async fn desktop(
    fixture: &Fixture,
    origin: &str,
    name: &str,
    key: &str,
) -> (Arc<Host>, Arc<MockDesktop>) {
    let mock = MockDesktop::start(fixture).await;
    mock.projects.lock().await[0]["label"] = json!(name);
    let port = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    ServiceConfig {
        port,
        bind: "127.0.0.1".into(),
        ..Default::default()
    }
    .save(&fixture.paths.data_home)
    .unwrap();
    let host = Host::open(fixture.paths.clone(), false).unwrap();
    host.configure_relay(origin, name, key).await.unwrap();
    host.start().await.unwrap();
    wait_online(&host).await;
    (host, mock)
}

async fn wait_online(host: &Host) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while host.status().await.unwrap()["relay"]["state"] != "online" {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
}

async fn invitation_link(host: &Host) -> String {
    host.relay_pairing().await.unwrap()["link"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn invitation(host: &Host) -> (Vec<u8>, String) {
    let link = invitation_link(host).await;
    let raw = link.split_once("#pair=").unwrap().1;
    let payload: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(raw).unwrap()).unwrap();
    (
        hex::decode(payload["publicKey"].as_str().unwrap()).unwrap(),
        payload["ticket"].as_str().unwrap().into(),
    )
}

struct EventStream {
    response: Response,
    buffer: BytesMut,
    batch: Bytes,
}

impl EventStream {
    fn new(response: Response) -> Self {
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        assert_eq!(
            response.headers()["cache-control"],
            "no-store, no-transform"
        );
        Self {
            response,
            buffer: BytesMut::new(),
            batch: Bytes::new(),
        }
    }

    async fn next(&mut self) -> Option<(String, String, String)> {
        loop {
            if let Some(end) = self.buffer.windows(2).position(|part| part == b"\n\n") {
                let block = self.buffer.split_to(end + 2);
                let text = std::str::from_utf8(&block).unwrap();
                let field = |prefix| {
                    text.lines()
                        .find_map(|line| line.strip_prefix(prefix))
                        .unwrap_or("")
                        .to_owned()
                };
                let kind = field("event: ");
                if kind != "heartbeat" {
                    return Some((kind, field("id: "), field("data: ")));
                }
                continue;
            }
            let chunk = self.response.chunk().await.unwrap()?;
            self.buffer.extend_from_slice(&chunk);
        }
    }

    async fn frame(&mut self) -> Option<Bytes> {
        if self.batch.is_empty() {
            let (event, _, data) = self.next().await?;
            assert_eq!(event, "frame");
            self.batch = URL_SAFE_NO_PAD.decode(data).unwrap().into();
        }
        let len = usize::from(self.batch.get_u16());
        Some(self.batch.split_to(len))
    }
}

struct Phone {
    client: Client,
    origin: String,
    cookie: String,
    endpoint: String,
    session: String,
    events: EventStream,
    cipher: snow::TransportState,
    next: u32,
}

impl Phone {
    async fn connect(
        origin: &str,
        cookie: &str,
        public: &[u8],
        private: &[u8],
        ticket: &str,
    ) -> (Self, bool) {
        let client = Client::new();
        let endpoint = format!(
            "{origin}/relay/mobile/{}",
            codex_mobile_bridge::digest(public)
        );
        let response = client
            .post(format!("{endpoint}/session"))
            .header("origin", origin)
            .header("cookie", cookie)
            .send()
            .await
            .unwrap();
        let mut events = EventStream::new(response);
        let (ready, _, session) = events.next().await.unwrap();
        assert_eq!(ready, "ready");

        let mut handshake = snow::Builder::new(NOISE.parse().unwrap())
            .prologue(PROLOGUE)
            .unwrap()
            .local_private_key(private)
            .unwrap()
            .remote_public_key(public)
            .unwrap()
            .build_initiator()
            .unwrap();
        let mut buffer = [0; 256];
        let length = handshake.write_message(&[], &mut buffer).unwrap();
        send_up(
            &client,
            origin,
            cookie,
            &endpoint,
            &session,
            &buffer[..length],
        )
        .await;
        assert_eq!(
            handshake
                .read_message(&events.frame().await.unwrap(), &mut buffer)
                .unwrap(),
            0
        );
        let mut phone = Self {
            client,
            origin: origin.into(),
            cookie: cookie.into(),
            endpoint,
            session,
            events,
            cipher: handshake.into_transport_mode().unwrap(),
            next: 0,
        };
        phone
            .send(
                1,
                0,
                &serde_json::to_vec(&json!({"name":"测试手机","ticket":ticket})).unwrap(),
            )
            .await;
        let (kind, id, reply) = phone.read().await.unwrap();
        assert_eq!((kind, id), (2, 0));
        (phone, reply == b"ok")
    }

    fn seal(&mut self, kind: u8, id: u32, content: &[u8]) -> Vec<u8> {
        let input = [&[kind], id.to_be_bytes().as_slice(), content].concat();
        let mut bytes = vec![0; input.len() + 16];
        let length = self.cipher.write_message(&input, &mut bytes).unwrap();
        bytes.truncate(length);
        bytes
    }

    async fn send_ciphertext(&self, bytes: &[u8]) {
        send_up(
            &self.client,
            &self.origin,
            &self.cookie,
            &self.endpoint,
            &self.session,
            bytes,
        )
        .await;
    }

    async fn send(&mut self, kind: u8, id: u32, content: &[u8]) {
        let bytes = self.seal(kind, id, content);
        self.send_ciphertext(&bytes).await;
    }

    async fn read(&mut self) -> Option<(u8, u32, Vec<u8>)> {
        let encrypted = self.events.frame().await?;
        let mut plain = vec![0; encrypted.len()];
        let size = self.cipher.read_message(&encrypted, &mut plain).unwrap();
        Some((
            plain[0],
            u32::from_be_bytes(plain[1..5].try_into().unwrap()),
            plain[5..size].to_vec(),
        ))
    }

    async fn request(&mut self, path: &str) -> (u16, Vec<u8>) {
        self.request_head(json!({"method":"GET","path":path,"length":0,"responseWindow":1}))
            .await
    }

    async fn request_head(&mut self, head: Value) -> (u16, Vec<u8>) {
        self.next += 1;
        let id = self.next;
        self.send(3, id, &serde_json::to_vec(&head).unwrap()).await;
        let (kind, reply_id, head) = self.read().await.unwrap();
        assert_eq!((kind, reply_id), (6, id));
        let status = serde_json::from_slice::<Value>(&head).unwrap()["status"]
            .as_u64()
            .unwrap() as u16;
        let mut body = Vec::new();
        loop {
            let (kind, reply_id, chunk) = self.read().await.unwrap();
            assert_eq!(reply_id, id);
            if kind == 8 {
                break;
            }
            assert_eq!(kind, 7);
            body.extend(chunk);
            self.send(10, id, &[1]).await;
        }
        (status, body)
    }
}

#[tokio::test]
async fn encrypted_relay_preserves_write_contract_without_upgrading_old_clients() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let (origin, server, owner) = relay().await;
        let cookie = login(&Client::new(), &origin, &owner.login_key).await;
        let fixture = Fixture::new();
        let (host, mock) = desktop(&fixture, &origin, "协议验证", &owner.desktop_key).await;
        let (public, ticket) = invitation(&host).await;
        let phone_key = snow::Builder::new(NOISE.parse().unwrap())
            .generate_keypair()
            .unwrap();
        let (mut phone, allowed) =
            Phone::connect(&origin, &cookie, &public, &phone_key.private, &ticket).await;
        assert!(allowed);
        for path in ["/api/threads", "/api/threads/chat/messages"] {
            for contract in [None, Some("1"), Some("2")] {
                let mut head = json!({"method":"POST","path":path,"length":0,"responseWindow":1});
                if let Some(contract) = contract {
                    head["contract"] = json!(contract);
                }
                let (status, body) = phone.request_head(head).await;
                // 当前协议进入业务能力校验，旧网页仍在投递前被拒绝。
                assert_eq!(status, if contract == Some("2") { 503 } else { 426 });
                assert_eq!(
                    serde_json::from_slice::<Value>(&body).unwrap()["outcome"],
                    "not-dispatched"
                );
            }
        }
        assert!(mock.calls.lock().await.is_empty());
        host.stop().await.unwrap();
        server.abort();
    })
    .await
    .unwrap();
}

async fn send_up(
    client: &Client,
    origin: &str,
    cookie: &str,
    endpoint: &str,
    session: &str,
    frame: &[u8],
) {
    let mut body = u16::try_from(frame.len()).unwrap().to_be_bytes().to_vec();
    body.extend_from_slice(frame);
    let response = client
        .post(format!("{endpoint}/send"))
        .header("origin", origin)
        .header("cookie", cookie)
        .header(SESSION_HEADER, session)
        .header("content-type", "application/octet-stream")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
}

#[tokio::test]
async fn encrypted_relay_rejects_tampering_replay_and_revoked_phones() {
    tokio::time::timeout(Duration::from_secs(25), async {
        let (origin, server, owner) = relay().await;
        let cookie = login(&Client::new(), &origin, &owner.login_key).await;
        let fixture_a = Fixture::new();
        let fixture_b = Fixture::new();
        let (a, _mock_a) = desktop(&fixture_a, &origin, "桌面甲项目", &owner.desktop_key).await;
        let (b, _mock_b) = desktop(&fixture_b, &origin, "桌面乙项目", &owner.desktop_key).await;
        let (key_a, ticket_a) = invitation(&a).await;
        let (key_b, ticket_b) = invitation(&b).await;
        assert_ne!(key_a, key_b);
        let phone_key = || {
            snow::Builder::new(NOISE.parse().unwrap())
                .generate_keypair()
                .unwrap()
        };
        let phone_a = phone_key();
        let phone_b = phone_key();
        let (mut client_a, allowed) =
            Phone::connect(&origin, &cookie, &key_a, &phone_a.private, &ticket_a).await;
        assert!(allowed);
        let (_, reused) =
            Phone::connect(&origin, &cookie, &key_a, &phone_key().private, &ticket_a).await;
        assert!(!reused);
        let (mut client_b, allowed) =
            Phone::connect(&origin, &cookie, &key_b, &phone_b.private, &ticket_b).await;
        assert!(allowed);
        for (fixture, label) in [(&fixture_a, "桌面甲项目"), (&fixture_b, "桌面乙项目")] {
            rusqlite::Connection::open(fixture.paths.codex_home.join("state_5.sqlite"))
                .unwrap()
                .execute("UPDATE threads SET title=?", [label])
                .unwrap();
        }
        for (phone, label) in [(&mut client_a, "桌面甲项目"), (&mut client_b, "桌面乙项目")]
        {
            let (status, bytes) = phone.request("/api/threads").await;
            assert_eq!(status, 200);
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).unwrap()["threads"][0]["title"],
                label
            );
        }
        a.revoke_relay_phone(&codex_mobile_bridge::digest(&phone_a.public))
            .await
            .unwrap();
        assert!(client_a.read().await.is_none());
        let (_, allowed) = Phone::connect(&origin, &cookie, &key_a, &phone_a.private, "").await;
        assert!(!allowed);
        assert_eq!(client_b.request("/api/session").await.0, 200);

        let (mut sender, allowed) =
            Phone::connect(&origin, &cookie, &key_b, &phone_b.private, "").await;
        assert!(allowed);
        let (mut recipient, allowed) =
            Phone::connect(&origin, &cookie, &key_b, &phone_b.private, "").await;
        assert!(allowed);
        let ciphertext = sender.seal(
            3,
            1,
            br#"{"method":"GET","path":"/api/session","length":0}"#,
        );
        recipient.send_ciphertext(&ciphertext).await;
        assert!(recipient.read().await.is_none());
        let mut corrupted = ciphertext;
        corrupted[7] ^= 1;
        sender.send_ciphertext(&corrupted).await;
        assert!(sender.read().await.is_none());

        b.stop().await.unwrap();
        assert!(client_b.read().await.is_none());
        b.start().await.unwrap();
        wait_online(&b).await;
        let (mut reconnected, allowed) =
            Phone::connect(&origin, &cookie, &key_b, &phone_b.private, "").await;
        assert!(allowed);
        assert_eq!(reconnected.request("/api/session").await.0, 200);
        a.stop().await.unwrap();
        b.stop().await.unwrap();
        server.abort();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn sse_relay_preserves_frames_and_releases_disconnected_streams() {
    tokio::time::timeout(Duration::from_secs(12), async {
        let (origin, server, owner) = relay().await;
        let client = Client::new();
        let cookie = login(&client, &origin, &owner.login_key).await;
        let id = "a".repeat(64);
        let desktop = format!("{origin}/relay/desktop/{id}");
        let mobile = format!("{origin}/relay/mobile/{id}/session");
        let mut control = register(&client, &origin, &id, &owner.desktop_key, "测试电脑").await;
        let bad = client
            .post(&mobile)
            .header("origin", "https://other.invalid")
            .send()
            .await
            .unwrap();
        assert_eq!(bad.status(), 403);
        for _ in 0..36 {
            let mut stream = EventStream::new(
                client
                    .post(&mobile)
                    .header("origin", &origin)
                    .header("cookie", &cookie)
                    .send()
                    .await
                    .unwrap(),
            );
            let (_, _, session) = stream.next().await.unwrap();
            assert_eq!(
                control.next().await.unwrap(),
                ("open".into(), "".into(), session.clone())
            );
            send_up(
                &client,
                &origin,
                &cookie,
                &format!("{origin}/relay/mobile/{id}"),
                &session,
                b"opaque frame",
            )
            .await;
            let (event, routed, body) = control.next().await.unwrap();
            assert_eq!(
                (event.as_str(), routed.as_str()),
                ("frame", session.as_str())
            );
            assert_eq!(
                URL_SAFE_NO_PAD.decode(body).unwrap(),
                b"\x00\x0copaque frame"
            );
            drop(stream);
            assert_eq!(
                control.next().await.unwrap(),
                ("close".into(), "".into(), session.clone())
            );
            let stale = client
                .post(format!("{desktop}/send"))
                .header(SESSION_HEADER, session)
                .bearer_auth(&owner.desktop_key)
                .body("stale")
                .send()
                .await
                .unwrap();
            assert_eq!(stale.status(), 404);
        }
        server.abort();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn accounts_isolate_directory_and_sessions_across_restart() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("relay.sqlite3");
        let accounts = Accounts::open(&db).unwrap();
        let owner = accounts.create("个人账号甲").unwrap();
        let other = accounts.create("个人账号乙").unwrap();
        let (origin, server) = relay_server(accounts).await;
        let client = Client::new();
        let a = login(&client, &origin, &owner.login_key).await;
        let b = login(&client, &origin, &other.login_key).await;
        let id_a = "a".repeat(64);
        let id_b = "b".repeat(64);
        let id_c = "c".repeat(64);
        let mut desktop_a = register(&client, &origin, &id_a, &owner.desktop_key, "工作电脑").await;
        let desktop_b = register(&client, &origin, &id_b, &owner.desktop_key, "家用电脑").await;
        let desktop_c = register(&client, &origin, &id_c, &other.desktop_key, "其他账号电脑").await;
        let endpoint = format!("{origin}/relay/account");
        let get_account = |cookie: &str| client.get(&endpoint).header("cookie", cookie);
        assert_eq!(client.get(&endpoint).send().await.unwrap().status(), 401);
        let response = get_account(&a).send().await.unwrap();
        assert_eq!(response.headers()["cache-control"], "private, no-store");
        let view: Value = response.json().await.unwrap();
        let names: std::collections::BTreeSet<_> = view["desktops"]
            .as_array()
            .unwrap()
            .iter()
            .map(|desktop| desktop["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["工作电脑", "家用电脑"].into_iter().collect());
        assert!(view["desktops"]
            .as_array()
            .unwrap()
            .iter()
            .all(|desktop| desktop["online"] == true));
        let second: Value = get_account(&b).send().await.unwrap().json().await.unwrap();
        assert_eq!(
            second["desktops"],
            json!([{"name":"其他账号电脑","id":id_c,"online":true}])
        );
        assert_ne!(second["id"], view["id"]);

        let mobile = format!("{origin}/relay/mobile/{id_a}/session");
        let open = || client.post(&mobile).header("origin", &origin);
        assert_eq!(open().send().await.unwrap().status(), 401);
        assert_eq!(
            open().header("cookie", &b).send().await.unwrap().status(),
            404
        );
        assert_eq!(
            client
                .post(&endpoint)
                .header("origin", &origin)
                .json(&json!({"loginKey": owner.desktop_key}))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(
            client
                .get(format!("{origin}/relay/desktop/{id_a}"))
                .bearer_auth(&owner.login_key)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        assert_eq!(
            client
                .delete(&endpoint)
                .header("origin", "https://other.invalid")
                .header("cookie", &a)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );

        let mut stream = EventStream::new(open().header("cookie", &a).send().await.unwrap());
        let (_, _, session) = stream.next().await.unwrap();
        assert_eq!(desktop_a.next().await.unwrap().0, "open");
        let fresh = login(&client, &origin, &owner.login_key).await;
        assert_eq!(
            client
                .post(format!("{origin}/relay/mobile/{id_a}/send"))
                .header("origin", &origin)
                .header("cookie", &fresh)
                .header(SESSION_HEADER, &session)
                .body("another browser")
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
        assert_eq!(
            client
                .post(format!("{origin}/relay/desktop/{id_a}/send"))
                .bearer_auth(&other.desktop_key)
                .header(SESSION_HEADER, &session)
                .body("another account")
                .send()
                .await
                .unwrap()
                .status(),
            404
        );

        assert_eq!(
            client
                .delete(&endpoint)
                .header("origin", &origin)
                .header("cookie", &a)
                .send()
                .await
                .unwrap()
                .status(),
            204
        );
        assert!(stream.next().await.is_none());
        assert_eq!(desktop_a.next().await.unwrap().0, "close");
        assert_eq!(get_account(&a).send().await.unwrap().status(), 401);
        drop((desktop_a, desktop_b, desktop_c));
        server.abort();

        let (restarted, server) = relay_server(Accounts::open(&db).unwrap()).await;
        let restored: Value = client
            .get(format!("{restarted}/relay/account"))
            .header("cookie", &fresh)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(restored["id"], view["id"]);
        let desktops = restored["desktops"].as_array().unwrap();
        assert_eq!(desktops.len(), 2);
        assert!(desktops.iter().all(|desktop| desktop["online"] == false));
        server.abort();
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "通过 npm run test:relay 使用实际手机客户端"]
async fn relay_serves_concurrent_mobile_clients() {
    tokio::time::timeout(Duration::from_secs(50), async {
        let (origin, relay, owner) = relay().await;
        let cookie = login(&Client::new(), &origin, &owner.login_key).await;
        let fixture_a = Fixture::new();
        let fixture_b = Fixture::new();
        let labels = ["relay-test-desktop-A", "relay-test-desktop-B"];
        let mut hosts = Vec::new();
        let mut mock_servers = Vec::new();
        let mut invitations = Vec::new();
        for (fixture, label) in [(&fixture_a, labels[0]), (&fixture_b, labels[1])] {
            let (host, mock) = desktop(fixture, &origin, label, &owner.desktop_key).await;
            let mut thread = support::official_thread("chat", false);
            thread["thread"]["turns"][0]["items"][0]["text"] = json!(label);
            mock.set_thread("chat", thread).await;
            rusqlite::Connection::open(fixture.paths.codex_home.join("state_5.sqlite"))
                .unwrap()
                .execute("UPDATE threads SET title=?", [label])
                .unwrap();
            rusqlite::Connection::open(fixture.paths.codex_home.join("thread_history_1.sqlite"))
                .unwrap()
                .execute(
                    "UPDATE thread_items SET item_json=? WHERE item_id='command-1'",
                    [json!({"id":"command-1","type":"agentMessage","text":label}).to_string()],
                )
                .unwrap();
            invitations.push(invitation_link(&host).await);
            hosts.push(host);
            mock_servers.push(mock);
        }
        let node = std::env::var_os("CODEX_MOBILE_TEST_NODE").expect("请运行 npm run test:relay");
        let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut child = tokio::process::Command::new(node)
            .args([
                "--disable-warning=ExperimentalWarning",
                "tests/fixtures/relay-client.mjs",
            ])
            .current_dir(project)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = serde_json::to_vec(
            &json!({"origin":origin,"cookie":cookie,"invitations":invitations,"labels":labels}),
        )
        .unwrap();
        child.stdin.take().unwrap().write_all(&input).await.unwrap();
        let output = child.wait_with_output().await.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8_lossy(&output.stdout));
        for host in hosts {
            host.stop().await.unwrap();
        }
        relay.abort();
    })
    .await
    .unwrap();
}
