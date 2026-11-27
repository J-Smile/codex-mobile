mod support;

use codex_mobile_bridge::{
    config::ServiceConfig,
    http::{Bridge, Server},
};
use rusqlite::Connection;
use serde_json::{json, Value};
use support::{Fixture, MockDesktop};

#[test]
fn archived_catalog_preserves_archive_state_and_rejects_active_cursor() {
    let f = Fixture::new();
    let connection = Connection::open(f.paths.codex_home.join("state_5.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE threads SET archived=1 WHERE id IN ('chat','other')",
            [],
        )
        .unwrap();
    let history = codex_mobile_bridge::history::History::new(f.paths.codex_home.clone());
    let archived = history.archived_catalog("", None, 1).unwrap();
    assert_eq!(archived["threads"].as_array().unwrap().len(), 1);
    assert!(archived["threads"]
        .as_array()
        .unwrap()
        .iter()
        .all(|thread| thread["archived"] == true));
    let cursor = archived["nextCursor"].as_str().unwrap();
    assert!(history.catalog(None, "", Some(cursor), 1).is_err());
    assert_eq!(history.metadata("chat", false).unwrap()["archived"], true);
}

async fn server(fixture: &Fixture) -> Server {
    Bridge::open(
        fixture.paths.clone(),
        ServiceConfig {
            port: 0,
            bind: "127.0.0.1".into(),
            ..Default::default()
        },
        false,
    )
    .unwrap()
    .start()
    .await
    .unwrap()
}

async fn login(server: &Server, client: &reqwest::Client) -> String {
    let response = client
        .post(format!("http://{}/api/login", server.address))
        .json(&json!({"accessKey":server.app.auth.lock().await.key()}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn history_catalog_never_claims_native_projects_or_opens_a_runtime() {
    let fixture = Fixture::new();
    let mock = MockDesktop::start(&fixture).await;
    Connection::open(fixture.paths.codex_home.join("state_5.sqlite"))
        .unwrap()
        .execute_batch("DROP TABLE projects; DROP TABLE project_roots;")
        .unwrap();
    let before = std::fs::read(fixture.paths.codex_home.join(".codex-global-state.json")).unwrap();
    let server = server(&fixture).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}", server.address);
    assert_eq!(
        client
            .get(format!("{url}/api/threads"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let cookie = login(&server, &client).await;
    for path in ["projects", "threads", "thread-catalog"] {
        let response = client
            .get(format!("{url}/api/{path}"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let page: Value = response.json().await.unwrap();
        assert_eq!(page["historyOnly"], true);
        if path == "projects" {
            assert_eq!(page["projects"], json!([]));
        } else {
            assert!(page["threads"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["projectId"].is_null()));
        }
    }
    let response = client
        .get(format!(
            "{url}/api/thread-catalog?projectId=guessed-from-cwd"
        ))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    assert!(mock.calls.lock().await.is_empty());
    assert_eq!(
        std::fs::read(fixture.paths.codex_home.join(".codex-global-state.json")).unwrap(),
        before
    );
    server.stop().await.unwrap();
}

#[tokio::test]
async fn history_catalog_groups_by_latest_saved_desktop_projects_without_runtime() {
    let fixture = Fixture::new();
    let mock = MockDesktop::start(&fixture).await;
    let root = &fixture.paths.codex_home;
    let state = Connection::open(root.join("state_5.sqlite")).unwrap();
    let cwd: String = state
        .query_row("SELECT cwd FROM threads WHERE id='chat'", [], |row| {
            row.get(0)
        })
        .unwrap();
    let project = json!({"desktop-project":{"name":"桌面项目","rootPaths":[cwd]}});
    std::fs::write(
        root.join(".codex-global-state.json"),
        json!({
            "local-projects":{"stale":{"name":"旧项目","rootPaths":[cwd]}},
            "thread-project-assignments":{"other":{"projectKind":"local","projectId":"stale"}},
        })
        .to_string(),
    )
    .unwrap();
    let settings = Connection::open(root.join(".codex-global-state.sqlite")).unwrap();
    settings
        .execute_batch(
            "CREATE TABLE settings (scope TEXT, key TEXT, value TEXT, PRIMARY KEY(scope,key));",
        )
        .unwrap();
    for (key, value) in [
        ("local-projects", project),
        ("project-order", json!(["desktop-project"])),
        (
            "thread-project-assignments",
            json!({"chat":{"projectKind":"local","projectId":"desktop-project"}}),
        ),
        (
            "thread-project-membership-host-ids",
            json!({"chat":"local"}),
        ),
    ] {
        settings
            .execute(
                "INSERT INTO settings VALUES ('global',?,?)",
                rusqlite::params![key, value.to_string()],
            )
            .unwrap();
    }
    drop(settings);
    let server = server(&fixture).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}", server.address);
    let cookie = login(&server, &client).await;
    for (path, ids) in [
        ("projects", vec!["desktop-project"]),
        ("threads", vec!["other", "chat"]),
        ("thread-catalog?projectId=desktop-project", vec!["chat"]),
    ] {
        let response = client
            .get(format!("{url}/api/{path}"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let page: Value = response.json().await.unwrap();
        assert_eq!(page["historyOnly"], true);
        if path == "projects" {
            assert_eq!(page["capabilities"]["catalog"], false);
            let projects = page["projects"].as_array().unwrap();
            assert_eq!(
                projects
                    .iter()
                    .map(|row| row["projectId"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                ids
            );
            assert_eq!(projects[0]["label"], "桌面项目");
        } else {
            let threads = page["threads"].as_array().unwrap();
            assert_eq!(
                threads
                    .iter()
                    .map(|row| row["id"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                ids
            );
            assert!(threads.iter().all(|row| row["hostId"].is_null()));
            for row in threads {
                assert_eq!(
                    row["projectId"],
                    if row["id"] == "chat" {
                        json!("desktop-project")
                    } else {
                        Value::Null
                    }
                );
            }
        }
    }
    assert!(mock.calls.lock().await.is_empty());
    server.stop().await.unwrap();
}

#[test]
fn shared_history_lists_cli_and_desktop_once_without_guessing_execution_owner() {
    use codex_mobile_bridge::history::History;
    let fixture = Fixture::new();
    let database = Connection::open(fixture.paths.codex_home.join("state_5.sqlite")).unwrap();
    database.execute_batch("UPDATE threads SET title='同名聊天'; UPDATE threads SET source='cli',updated_at_ms=(SELECT MAX(updated_at_ms) FROM threads)+1 WHERE id='other';").unwrap();
    std::fs::write(
        fixture.paths.codex_home.join(".codex-global-state.json"),
        json!({
            "pinned-thread-ids":["chat","other","chat"],
            "local-projects":{"desktop":{"name":"样例项目","rootPaths":["/fixture/project"]}},
            "thread-project-assignments":{"chat":{"projectKind":"local","projectId":"desktop"}},
        })
        .to_string(),
    )
    .unwrap();
    let history = History::new(fixture.paths.codex_home.clone());
    assert_eq!(
        history.default_thread_id().unwrap().as_deref(),
        Some("other")
    );
    let page = history.thread_list(None).unwrap();
    let rows = page["threads"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row["hostId"].is_null()));
    for (id, source) in [("chat", "vscode"), ("other", "cli")] {
        let row = rows.iter().find(|row| row["id"] == id).unwrap();
        assert_eq!(row["source"], source);
        assert_eq!(row["historyMode"], "paginated");
        assert_eq!(history.metadata(id, false).unwrap()["source"], source);
        assert_eq!(
            database
                .query_row("SELECT source FROM threads WHERE id=?", [id], |row| row
                    .get::<_, String>(
                    0
                ))
                .unwrap(),
            source
        );
    }
    assert_eq!(history.projects().unwrap()[0]["projectId"], "desktop");
    let project = history.catalog(Some("desktop"), "", None, 1).unwrap();
    assert_eq!(project["threads"][0]["id"], "chat");
    assert!(project["nextCursor"].is_null());
    let first = history.catalog(None, "", None, 1).unwrap();
    let next = history
        .catalog(None, "", first["nextCursor"].as_str(), 1)
        .unwrap();
    assert_ne!(first["threads"][0]["id"], next["threads"][0]["id"]);
    assert!(next["nextCursor"].is_null());
}

#[tokio::test]
async fn unscoped_desktop_tools_do_not_borrow_or_create_a_control_chat() {
    use codex_mobile_bridge::{desktop::Desktop, history::History};
    let fixture = Fixture::new();
    let mock = MockDesktop::start(&fixture).await;
    let desktop = Desktop::new(
        fixture.paths.clone(),
        std::sync::Arc::new(History::new(fixture.paths.codex_home.clone())),
    );
    let error = desktop
        .tools_call("list_threads", json!({}), None, None)
        .await
        .unwrap_err();
    assert!(error.before_send);
    assert!(error.message.contains("明确的目标聊天"));
    assert!(mock.calls.lock().await.is_empty());
    desktop.close().await;
}
