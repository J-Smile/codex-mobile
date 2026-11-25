#[cfg(unix)]
use codex_mobile_bridge::app_server::connect_websocket;
use codex_mobile_bridge::app_server::{AppServerConnection, AppServerEvent};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::broadcast,
};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

async fn read_json<S: AsyncRead + AsyncWrite + Unpin>(socket: &mut WebSocketStream<S>) -> Value {
    let message = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(message.to_text().unwrap()).unwrap()
}
async fn send_json<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    value: Value,
) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
}
async fn initialize<S: AsyncRead + AsyncWrite + Unpin>(socket: &mut WebSocketStream<S>) {
    let request = read_json(socket).await;
    assert_eq!(request["method"], "initialize");
    assert_eq!(request["params"]["clientInfo"]["name"], "codex-mobile");
    assert_eq!(request["params"]["capabilities"]["experimentalApi"], true);
    assert_eq!(
        request["params"]["capabilities"]["mcpServerOpenaiFormElicitation"],
        true
    );
    send_json(socket, json!({"id": request["id"], "result": {"userAgent": "official-rpc-fixture", "codexHome": "/fixture/codex"}})).await;
    let initialized = read_json(socket).await;
    assert_eq!(initialized["method"], "initialized");
    assert!(initialized.get("id").is_none());
}
async fn next_event(receiver: &mut broadcast::Receiver<AppServerEvent>) -> AppServerEvent {
    tokio::time::timeout(Duration::from_secs(3), receiver.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn one_websocket_multiplexes_threads_notifications_and_original_approval_ids() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}", listener.local_addr().unwrap());
    let notification = json!({"method": "item/agentMessage/delta", "params": {"threadId": "one", "delta": "fixture-text"}});
    let expected_notification = notification.clone();
    let fixture = tokio::spawn(async move {
        let mut socket = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
            .await
            .unwrap();
        initialize(&mut socket).await;
        let first = read_json(&mut socket).await;
        let second = read_json(&mut socket).await;
        assert_eq!(first["method"], "thread/read");
        assert_eq!(second["method"], "thread/read");
        assert_ne!(first["id"], second["id"]);
        assert_ne!(first["params"]["threadId"], second["params"]["threadId"]);
        send_json(&mut socket, notification).await;
        // Bidirectional JSON-RPC ids have separate namespaces: this request
        // deliberately collides with an outstanding client request id.
        for id in [first["id"].clone(), json!(731)] {
            send_json(&mut socket, json!({
                "id": id, "method": "mcpServer/elicitation/request",
                "params": {"threadId": "one", "serverName": "fixture", "mode": "form", "message": "Allow fixture?", "requestedSchema": {"type": "object", "properties": {}}}
            })).await;
        }
        let approvals = [read_json(&mut socket).await, read_json(&mut socket).await];
        assert_eq!(approvals[0]["id"], first["id"]);
        assert_eq!(approvals[1]["id"], 731);
        assert!(approvals[0]["id"].is_string());
        assert!(approvals[1]["id"].is_number());
        for approval in approvals {
            assert!(approval.get("method").is_none());
            assert_eq!(approval["result"], json!({"action": "accept"}));
        }
        for request in [second, first] {
            send_json(&mut socket, json!({"id": request["id"], "result": {"thread": {"id": request["params"]["threadId"]}}})).await;
        }
        socket
            .send(Message::Ping(vec![1, 2, 3].into()))
            .await
            .unwrap();
        let pong = tokio::time::timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(pong, Message::Pong(vec![1, 2, 3].into()));
        socket.close(None).await.unwrap();
    });
    let connection = AppServerConnection::connect(&endpoint).await.unwrap();
    assert_eq!(connection.initialized["userAgent"], "official-rpc-fixture");
    let other = connection.clone();
    assert!(Arc::ptr_eq(&connection, &other));
    let mut first_subscriber = connection.subscribe();
    let mut second_subscriber = connection.subscribe();
    let one = tokio::spawn(async move {
        other
            .rpc_with_sequence("thread/read", json!({"threadId": "one"}))
            .await
    });
    let other = connection.clone();
    let two = tokio::spawn(async move {
        other
            .request_with_sequence(
                "thread/read",
                json!({"threadId": "two"}),
                Duration::from_secs(3),
            )
            .await
    });
    let mut last_sequence = 0;
    for receiver in [&mut first_subscriber, &mut second_subscriber] {
        match next_event(receiver).await {
            AppServerEvent::Notification {
                connection_id,
                sequence,
                message,
            } => {
                assert_eq!(connection_id, connection.id);
                assert_eq!(message, expected_notification);
                assert!(sequence > 0);
                if last_sequence != 0 {
                    assert_eq!(sequence, last_sequence);
                }
                last_sequence = sequence;
            }
            event => panic!("expected notification, got {event:?}"),
        }
    }
    for _ in 0..2 {
        let first = next_event(&mut first_subscriber).await;
        let second = next_event(&mut second_subscriber).await;
        match (first, second) {
            (
                AppServerEvent::ServerRequest {
                    connection_id,
                    sequence,
                    message,
                },
                AppServerEvent::ServerRequest {
                    sequence: duplicate_sequence,
                    message: duplicate,
                    ..
                },
            ) => {
                assert_eq!(connection_id, connection.id);
                assert_eq!(message, duplicate);
                assert_eq!(sequence, duplicate_sequence);
                assert!(sequence > last_sequence);
                last_sequence = sequence;
                connection
                    .respond(message["id"].clone(), json!({"action": "accept"}))
                    .await
                    .unwrap();
            }
            events => panic!("expected approval broadcast, got {events:?}"),
        }
    }
    let (one, one_sequence) = one.await.unwrap().unwrap();
    let (two, two_sequence) = two.await.unwrap().unwrap();
    assert_eq!(one["thread"]["id"], "one");
    assert_eq!(two["thread"]["id"], "two");
    assert_ne!(one_sequence, two_sequence);
    assert!(one_sequence > last_sequence && two_sequence > last_sequence);
    match next_event(&mut first_subscriber).await {
        AppServerEvent::Disconnected { sequence, .. } => {
            assert!(sequence > one_sequence.max(two_sequence))
        }
        event => panic!("expected disconnect, got {event:?}"),
    }
    assert!(!connection.is_alive());
    fixture.await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn raw_unix_websocket_decodes_socket_paths_and_does_not_initialize_rpc() {
    let temporary = tempfile::Builder::new()
        .prefix("cm-ws-")
        .tempdir_in("/tmp")
        .unwrap();
    let socket_path = temporary.path().join("rpc socket.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
    let endpoint = format!(
        "unix://{}",
        socket_path.to_string_lossy().replace(' ', "%20")
    );
    let fixture = tokio::spawn(async move {
        let mut socket = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
            .await
            .unwrap();
        assert_eq!(read_json(&mut socket).await, json!({"fixture": "raw"}));
        send_json(&mut socket, json!({"raw": "ok"})).await;
    });
    let mut socket = connect_websocket(&endpoint).await.unwrap();
    send_json(&mut socket, json!({"fixture": "raw"})).await;
    assert_eq!(read_json(&mut socket).await, json!({"raw": "ok"}));
    fixture.await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn official_rpc_initializes_and_reuses_a_unix_websocket() {
    let temporary = tempfile::Builder::new()
        .prefix("cm-ws-")
        .tempdir_in("/tmp")
        .unwrap();
    let socket_path = temporary.path().join("rpc.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
    let endpoint = format!("unix://{}", socket_path.display());
    let fixture = tokio::spawn(async move {
        let mut socket = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
            .await
            .unwrap();
        initialize(&mut socket).await;
        for method in ["thread/resume", "thread/turns/list"] {
            let request = read_json(&mut socket).await;
            assert_eq!(request["method"], method);
            send_json(
                &mut socket,
                json!({"id": request["id"], "result": {"method": method}}),
            )
            .await;
        }
    });
    let connection = AppServerConnection::connect(&endpoint).await.unwrap();
    for method in ["thread/resume", "thread/turns/list"] {
        assert_eq!(
            connection
                .rpc(method, json!({"threadId": "fixture"}))
                .await
                .unwrap()["method"],
            method
        );
    }
    fixture.await.unwrap();
}

#[tokio::test]
async fn disconnect_marks_sent_mutations_uncertain_and_never_reconnects_or_replays() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}", listener.local_addr().unwrap());
    let fixture = tokio::spawn(async move {
        let mut socket = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
            .await
            .unwrap();
        initialize(&mut socket).await;
        let mutation = read_json(&mut socket).await;
        assert_eq!(mutation["method"], "turn/start");
        drop(socket);
        // Keep the listener available to detect an unsolicited reconnect.
        assert!(
            tokio::time::timeout(Duration::from_millis(150), listener.accept())
                .await
                .is_err()
        );
        mutation
    });
    let connection = AppServerConnection::connect(&endpoint).await.unwrap();
    let mut events = connection.subscribe();
    let error = connection
        .rpc("turn/start", json!({"threadId": "fixture", "input": []}))
        .await
        .unwrap_err();
    assert!(!error.before_send);
    assert!(error.rpc_error.is_none());
    assert!(matches!(
        next_event(&mut events).await,
        AppServerEvent::Disconnected { .. }
    ));
    assert!(!connection.is_alive());
    let not_sent = connection
        .rpc("turn/start", json!({"threadId": "fixture", "input": []}))
        .await
        .unwrap_err();
    assert!(not_sent.before_send);
    assert!(not_sent.rpc_error.is_none());
    let stale_reply = connection
        .respond(json!(731), json!({"action": "accept"}))
        .await
        .unwrap_err();
    assert!(stale_reply.before_send);
    assert_eq!(fixture.await.unwrap()["method"], "turn/start");
}

#[tokio::test]
async fn late_responses_after_timeout_do_not_complete_a_later_rpc() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}", listener.local_addr().unwrap());
    let (received, observed) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let fixture = tokio::spawn(async move {
        let mut socket = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
            .await
            .unwrap();
        initialize(&mut socket).await;
        let first = read_json(&mut socket).await;
        received.send(()).unwrap();
        released.await.unwrap();
        send_json(
            &mut socket,
            json!({"id": first["id"], "result": {"unexpected": "late"}}),
        )
        .await;
        let second = read_json(&mut socket).await;
        assert_ne!(first["id"], second["id"]);
        send_json(
            &mut socket,
            json!({"id": second["id"], "result": {"current": "ok"}}),
        )
        .await;
    });
    let connection = AppServerConnection::connect(&endpoint).await.unwrap();
    let other = connection.clone();
    let request = tokio::spawn(async move {
        other
            .request(
                "thread/read",
                json!({"threadId": "one"}),
                Duration::from_millis(100),
            )
            .await
    });
    observed.await.unwrap();
    let error = request.await.unwrap().unwrap_err();
    assert!(!error.before_send);
    assert!(error.rpc_error.is_none());
    assert!(connection.is_alive());
    release.send(()).unwrap();
    assert_eq!(
        connection
            .rpc("thread/read", json!({"threadId": "two"}))
            .await
            .unwrap(),
        json!({"current": "ok"})
    );
    fixture.await.unwrap();
}

#[tokio::test]
async fn rpc_error_preserves_details_without_claiming_no_side_effects() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}", listener.local_addr().unwrap());
    let fixture = tokio::spawn(async move {
        let mut socket = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
            .await
            .unwrap();
        initialize(&mut socket).await;
        let invalid = read_json(&mut socket).await;
        send_json(
            &mut socket,
            json!({"id": invalid["id"], "error": {"code": -32602, "message": "fixture rejected"}}),
        )
        .await;
        let valid = read_json(&mut socket).await;
        send_json(
            &mut socket,
            json!({"id": valid["id"], "result": {"ok": true}}),
        )
        .await;
    });
    let connection = AppServerConnection::connect(&endpoint).await.unwrap();
    let error = connection.rpc("thread/read", json!({})).await.unwrap_err();
    assert_eq!(error.message, "fixture rejected");
    assert!(error.rpc_error.is_some());
    assert!(!error.before_send);
    assert!(connection.is_alive());
    assert_eq!(
        connection
            .rpc("thread/read", json!({"threadId": "fixture"}))
            .await
            .unwrap(),
        json!({"ok": true})
    );
    fixture.await.unwrap();
}
