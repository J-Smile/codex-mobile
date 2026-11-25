//! Project and thread catalogs from Codex's official desktop tools.
use crate::{desktop::Desktop, BridgeError, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashSet, future::Future};

const NATIVE_PAGE_LIMIT: usize = 50;
const FILTERED_PAGE_READS: usize = 2;

pub async fn projects(desktop: &Desktop) -> Result<Vec<Value>> {
    normalize_projects(
        desktop
            .tools_call("list_projects", json!({}), None, None)
            .await?,
    )
}

pub fn normalize_projects(native_payload: Value) -> Result<Vec<Value>> {
    let payload = native_payload_value(native_payload)?;
    let mut projects = Vec::new();
    for project in array(&payload, "projects")? {
        if string(project, "projectKind")? != "local" {
            continue;
        }
        if string(project, "hostId")? != "local" {
            continue;
        }
        if string(project, "projectId")?.is_empty() {
            return Err(invalid_native());
        }
        string(project, "label")?;
        string(project, "path")?;
        projects.push(project.clone());
    }
    Ok(projects)
}

pub async fn thread_list(desktop: &Desktop, cursor: Option<&str>) -> Result<Value> {
    let recent = if cursor.is_none() {
        Some(normalize_recent(
            desktop
                .tools_call(
                    "list_threads",
                    json!({"mode":"recent","limit":50}),
                    None,
                    None,
                )
                .await?,
        )?)
    } else {
        None
    };
    let mut page = read_catalog(
        Scope::new(None, "")?,
        cursor,
        NATIVE_PAGE_LIMIT,
        1,
        |args| desktop.tools_call("list_threads", args, None, None),
    )
    .await?;
    if let Some(recent) = recent {
        let history = page["threads"].as_array().unwrap();
        page["threads"] = json!(merge_recent(recent, history));
    }
    Ok(page)
}

pub async fn catalog(
    desktop: &Desktop,
    project: Option<&Value>,
    query: &str,
    cursor: Option<&str>,
    limit: usize,
) -> Result<Value> {
    read_catalog(
        Scope::new(project, query)?,
        cursor,
        limit,
        FILTERED_PAGE_READS,
        |args| desktop.tools_call("list_threads", args, None, None),
    )
    .await
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Scope {
    project_id: Option<String>,
    query: String,
}

impl Scope {
    fn new(project: Option<&Value>, query: &str) -> Result<Self> {
        let project_id = project
            .map(|project| {
                project["projectId"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| BridgeError::invalid("聊天项目无效，请返回项目列表。"))
            })
            .transpose()?;
        Ok(Self {
            project_id,
            query: query.trim().to_lowercase(),
        })
    }

    fn matches(&self, thread: &Value) -> bool {
        self.project_id
            .as_deref()
            .is_none_or(|project_id| thread["projectId"].as_str() == Some(project_id))
            && thread["title"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains(&self.query)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Cursor {
    version: u8,
    scope: Scope,
    native_cursor: String,
}

fn cursor_position(cursor: Option<&str>, scope: &Scope) -> Result<Option<String>> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    let invalid = || BridgeError::invalid("聊天分页位置无效，请返回第一页。");
    let decoded = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| invalid())?;
    let cursor: Cursor = serde_json::from_slice(&decoded).map_err(|_| invalid())?;
    if cursor.version != 1 || cursor.scope != *scope || cursor.native_cursor.is_empty() {
        return Err(invalid());
    }
    Ok(Some(cursor.native_cursor))
}

async fn read_catalog<F, Fut>(
    scope: Scope,
    cursor: Option<&str>,
    limit: usize,
    max_reads: usize,
    mut fetch: F,
) -> Result<Value>
where
    F: FnMut(Value) -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    if limit == 0 {
        return Err(BridgeError::invalid("聊天分页数量必须大于零。"));
    }
    let mut position = cursor_position(cursor, &scope)?;
    let mut threads = Vec::new();
    let mut ids = HashSet::new();
    for _ in 0..max_reads {
        // Reading only the remaining count avoids consuming matches which
        // would otherwise have to be buffered after the native cursor.
        let native_limit = (limit - threads.len()).min(NATIVE_PAGE_LIMIT);
        let mut args = json!({
            "mode":"history", "source":"codex", "hostId":"local",
            "archived":false, "limit":native_limit
        });
        if let Some(cursor) = &position {
            args["cursor"] = json!(cursor);
        }
        let payload = native_payload_value(fetch(args).await?)?;
        if array(&payload, "threads")?.len() > native_limit {
            return Err(BridgeError::unavailable(
                "Codex 返回的聊天页超过请求数量，请重试。",
            ));
        }
        for thread in local_threads(&payload, "threads")? {
            if scope.matches(&thread) && ids.insert(thread["id"].as_str().unwrap().to_owned()) {
                threads.push(thread);
            }
        }
        let next = match payload.get("nextCursor") {
            Some(Value::Null) => None,
            Some(Value::String(cursor)) if !cursor.is_empty() => Some(cursor.clone()),
            _ => return Err(invalid_native()),
        };
        if next.is_some() && next == position {
            return Err(BridgeError::unavailable(
                "Codex 的聊天分页位置没有前进，请返回第一页。",
            ));
        }
        position = next;
        if position.is_none() || threads.len() == limit {
            break;
        }
    }
    let next_cursor = position.map(|native_cursor| {
        URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&Cursor {
                version: 1,
                scope,
                native_cursor,
            })
            .unwrap(),
        )
    });
    Ok(json!({"threads":threads,"nextCursor":next_cursor}))
}

fn normalize_recent(native_payload: Value) -> Result<Vec<Value>> {
    let payload = native_payload_value(native_payload)?;
    let mut pinned = local_threads(&payload, "pinnedThreads")?;
    pinned.sort_by_key(|thread| thread["pinnedIndex"].as_u64().unwrap_or(u64::MAX));
    for (index, thread) in pinned.iter_mut().enumerate() {
        if thread.get("pinnedIndex").is_none() {
            thread["pinnedIndex"] = json!(index + 1);
        }
    }
    pinned.extend(local_threads(&payload, "threads")?);
    Ok(pinned)
}

fn merge_recent(recent: Vec<Value>, history: &[Value]) -> Vec<Value> {
    let mut entries = indexmap::IndexMap::new();
    for thread in recent.into_iter().chain(history.iter().cloned()) {
        entries
            .entry(thread["id"].as_str().unwrap().to_owned())
            .or_insert(thread);
    }
    entries.into_values().collect()
}

fn local_threads(payload: &Value, field: &str) -> Result<Vec<Value>> {
    let mut threads = Vec::new();
    for thread in array(payload, field)? {
        if string(thread, "kind")? != "codex" {
            continue;
        }
        if string(thread, "hostId")? != "local" {
            continue;
        }
        if string(thread, "id")?.is_empty() {
            return Err(invalid_native());
        }
        string(thread, "title")?;
        if !matches!(
            thread.get("projectId"),
            Some(Value::Null | Value::String(_))
        ) {
            return Err(invalid_native());
        }
        threads.push(thread.clone());
    }
    Ok(threads)
}

fn native_payload_value(mut payload: Value) -> Result<Value> {
    if payload["isError"] == true {
        let message = content_text(&payload);
        return Err(BridgeError::unavailable(if message.is_empty() {
            "Codex 未能读取聊天目录。".to_owned()
        } else {
            message
        }));
    }
    if let Some(structured) = payload.get("structuredContent") {
        payload = structured.clone();
    } else if payload.get("content").is_some() {
        payload = serde_json::from_str(&content_text(&payload)).map_err(|_| invalid_native())?;
    }
    if let Some(error) = payload.get("error").filter(|error| !error.is_null()) {
        return Err(BridgeError::unavailable(
            error["message"]
                .as_str()
                .unwrap_or("Codex 未能读取聊天目录。"),
        ));
    }
    if !payload.is_object() {
        return Err(invalid_native());
    }
    Ok(payload)
}

fn content_text(payload: &Value) -> String {
    payload["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|content| content["type"] == "text")
        .filter_map(|content| content["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn array<'a>(payload: &'a Value, field: &str) -> Result<&'a Vec<Value>> {
    payload[field].as_array().ok_or_else(invalid_native)
}

fn string<'a>(payload: &'a Value, field: &str) -> Result<&'a str> {
    payload[field].as_str().ok_or_else(invalid_native)
}

fn invalid_native() -> BridgeError {
    BridgeError::unavailable("Codex 返回的聊天目录格式无效。")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn thread(id: &str, project_id: Value, title: &str) -> Value {
        json!({
            "id":id, "kind":"codex", "hostId":"local", "projectId":project_id,
            "cwd":"/same/workspace", "title":title, "summary":"different summary",
            "updatedAt":1794731992
        })
    }

    fn mcp(payload: Value) -> Value {
        json!({"content":[{"type":"text","text":payload.to_string()}],"isError":false})
    }

    #[test]
    fn projects_use_native_ids_and_order_for_local_projects() {
        let local = json!({
            "projectId":"native-project", "projectKind":"local", "hostId":"local",
            "label":"真实项目", "path":"/same/workspace", "isGitRepository":true
        });
        let another = json!({
            "projectId":"another-native", "projectKind":"local", "hostId":"local",
            "label":"另一个项目", "path":"/other"
        });
        let mut remote = local.clone();
        remote["hostId"] = json!("remote-host");
        let result = normalize_projects(mcp(json!({"projects":[
            another.clone(),remote,{"projectKind":"chatgpt","hostId":null},local.clone()
        ]})))
        .unwrap();
        assert_eq!(result, vec![another, local]);
    }

    #[tokio::test]
    async fn project_filter_keeps_null_out_despite_equal_cwd_and_preserves_title() {
        let wanted = thread("native-thread", json!("native-project"), "真实 100%_TITLE");
        let payload = mcp(json!({"threads":[
            thread("unassigned",Value::Null,"真实 100%_TITLE"),
            wanted.clone(),
            thread("raw-id",json!("raw-server-id"),"真实 100%_TITLE"),
            thread("summary-only",json!("native-project"),"another title")
        ],"nextCursor":null}));
        let page = read_catalog(
            Scope::new(
                Some(&json!({"projectId":"native-project","path":"/same/workspace"})),
                "100%_title",
            )
            .unwrap(),
            None,
            10,
            FILTERED_PAGE_READS,
            |_| std::future::ready(Ok(payload.clone())),
        )
        .await
        .unwrap();
        assert_eq!(page["threads"], json!([wanted]));
        assert!(page["nextCursor"].is_null());
    }

    #[tokio::test]
    async fn sparse_pages_are_bounded_and_remaining_limits_do_not_lose_matches() {
        let source = Arc::new(
            (0..13)
                .map(|index| {
                    thread(
                        &format!("row-{index}"),
                        if [6, 9, 10, 11].contains(&index) {
                            json!("project")
                        } else {
                            Value::Null
                        },
                        "title",
                    )
                })
                .collect::<Vec<_>>(),
        );
        let calls = Arc::new(Mutex::new(Vec::new()));
        let scope = Scope::new(Some(&json!({"projectId":"project"})), "").unwrap();
        let mut cursor = None;
        let mut found = Vec::new();
        let mut pages = 0;
        loop {
            let before = calls.lock().unwrap().len();
            let page = read_catalog(
                scope.clone(),
                cursor.as_deref(),
                3,
                FILTERED_PAGE_READS,
                |args| {
                    let offset = args["cursor"]
                        .as_str()
                        .map(|cursor| {
                            cursor
                                .strip_prefix("native:")
                                .unwrap()
                                .parse::<usize>()
                                .unwrap()
                        })
                        .unwrap_or(0);
                    let limit = args["limit"].as_u64().unwrap() as usize;
                    assert_eq!(args["mode"], "history");
                    assert_eq!(args["hostId"], "local");
                    assert_eq!(args["source"], "codex");
                    assert_eq!(args["archived"], false);
                    calls.lock().unwrap().push(limit);
                    let end = (offset + limit).min(source.len());
                    std::future::ready(Ok(mcp(json!({
                        "threads":source[offset..end],
                        "nextCursor":(end < source.len()).then(|| format!("native:{end}"))
                    }))))
                },
            )
            .await
            .unwrap();
            assert!(calls.lock().unwrap().len() - before <= FILTERED_PAGE_READS);
            if pages == 0 {
                assert!(page["threads"].as_array().unwrap().is_empty());
                assert!(page["nextCursor"].is_string());
            }
            found.extend(
                page["threads"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|thread| thread["id"].as_str().unwrap().to_owned()),
            );
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            pages += 1;
            if cursor.is_none() {
                break;
            }
            assert!(pages < 10);
        }
        assert_eq!(found, ["row-6", "row-9", "row-10", "row-11"]);
        assert_eq!(*calls.lock().unwrap(), vec![3, 3, 3, 2, 3]);
    }

    #[tokio::test]
    async fn cursor_preserves_native_position_and_rejects_other_scopes() {
        let scope = Scope::new(Some(&json!({"projectId":"native-project"})), " Hello ").unwrap();
        let native = "opaque+/cursor?with=值";
        let cursor = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&Cursor {
                version: 1,
                scope: scope.clone(),
                native_cursor: native.to_owned(),
            })
            .unwrap(),
        );
        assert_eq!(
            cursor_position(Some(&cursor), &scope).unwrap().as_deref(),
            Some(native)
        );
        let calls = Arc::new(Mutex::new(0));
        let page = read_catalog(
            scope.clone(),
            Some(&cursor),
            5,
            FILTERED_PAGE_READS,
            |args| {
                assert_eq!(args["cursor"], native);
                *calls.lock().unwrap() += 1;
                std::future::ready(Ok(json!({"threads":[],"nextCursor":null})))
            },
        )
        .await
        .unwrap();
        assert!(page["nextCursor"].is_null());
        assert_eq!(*calls.lock().unwrap(), 1);
        for other in [
            Scope::new(None, "hello").unwrap(),
            Scope::new(Some(&json!({"projectId":"another-project"})), "hello").unwrap(),
            Scope::new(Some(&json!({"projectId":"native-project"})), "different").unwrap(),
        ] {
            assert_eq!(
                cursor_position(Some(&cursor), &other).unwrap_err().status,
                400
            );
        }
        assert!(cursor_position(Some("invalid cursor"), &scope).is_err());
        assert_eq!(
            Scope::new(Some(&json!({"projectId":"native-project"})), "HELLO").unwrap(),
            scope
        );
    }

    #[test]
    fn recent_preserves_pins_and_filters_other_hosts_and_sources() {
        let mut first_pin = thread("pin-one", Value::Null, "真实置顶标题");
        first_pin["pinnedIndex"] = json!(1);
        let mut second_pin = thread("pin-two", json!("native-project"), "第二个置顶");
        second_pin["pinnedIndex"] = json!(2);
        let mut remote = thread("remote", Value::Null, "remote");
        remote["hostId"] = json!("remote-host");
        let recent = thread("recent", Value::Null, "近期标题");
        let pins = normalize_recent(mcp(json!({
            "pinnedThreads":[second_pin.clone(),first_pin.clone()],
            "threads":[remote,{"kind":"chatgpt","hostId":null},recent.clone(),first_pin.clone()]
        })))
        .unwrap();
        let older = thread("older", Value::Null, "较早标题");
        let merged = merge_recent(pins, &[recent.clone(), older.clone()]);
        assert_eq!(merged, vec![first_pin, second_pin, recent, older]);
        assert!(merged[0]["projectId"].is_null());
    }

    #[tokio::test]
    async fn native_errors_and_malformed_or_oversized_pages_are_not_empty_successes() {
        let error = native_payload_value(
            json!({"isError":true,"content":[{"type":"text","text":"native failure"}]}),
        )
        .unwrap_err();
        assert_eq!(error.message, "native failure");
        assert!(normalize_projects(json!({"message":"not a project list"})).is_err());
        assert!(
            native_payload_value(json!({"content":[{"type":"text","text":"not json"}]})).is_err()
        );
        for payload in [
            json!({"threads":[]}),
            json!({"threads":[thread("one",Value::Null,"title"),thread("two",Value::Null,"title")],"nextCursor":null}),
            json!({"error":{"message":"native failure"}}),
        ] {
            let result = read_catalog(
                Scope::new(None, "").unwrap(),
                None,
                1,
                FILTERED_PAGE_READS,
                |_| std::future::ready(Ok(payload.clone())),
            )
            .await;
            assert!(result.is_err());
        }
        let scope = Scope::new(None, "").unwrap();
        let cursor = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&Cursor {
                version: 1,
                scope: scope.clone(),
                native_cursor: "unchanged".to_owned(),
            })
            .unwrap(),
        );
        assert!(
            read_catalog(scope, Some(&cursor), 1, FILTERED_PAGE_READS, |_| {
                std::future::ready(Ok(json!({"threads":[],"nextCursor":"unchanged"})))
            })
            .await
            .is_err()
        );
    }
}
