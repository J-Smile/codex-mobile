use crate::{paths::read_json_optional, state::summarize_item, BridgeError, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use indexmap::IndexMap;
use rusqlite::{
    params, params_from_iter, types::Value as SqlValue, Connection, OpenFlags, OptionalExtension,
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Mutex,
};

const SUMMARY_FIELDS: &[&str] = &[
    "id",
    "type",
    "status",
    "tool",
    "server",
    "name",
    "path",
    "exitCode",
    "durationMs",
    "startedAtMs",
    "completedAtMs",
    "command",
    "commandActions",
    "changes",
    "kind",
    "agentThreadId",
    "agentPath",
    "model",
    "reasoningEffort",
    "receiverThreadIds",
];

pub struct History {
    pub codex_home: PathBuf,
    mode_cache: Mutex<HashMap<String, CachedMode>>,
}
struct CachedMode {
    size: u64,
    modified: std::time::SystemTime,
    mode: Option<String>,
}
struct ProjectState {
    projects: IndexMap<String, Value>,
    assignments: HashMap<String, Option<String>>,
    server_ids: HashMap<String, String>,
    desktop_ids: HashMap<String, String>,
    pins: Vec<Value>,
}

fn catalog_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let name: Option<String> = row.get(1)?;
    let title: Option<String> = row.get(2)?;
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "title": name.filter(|s| !s.is_empty()).or(title).unwrap_or_else(|| "未命名聊天".into()),
        "cwd": row.get::<_, String>(3)?,
        "model": row.get::<_, Option<String>>(4)?,
        "reasoningEffort": row.get::<_, Option<String>>(5)?,
        "projectId": row.get::<_, Option<String>>(6)?,
        "updatedAt": row.get::<_, i64>(7)?,
        "kind": "codex",
        "hostId": null,
        "source": row.get::<_, Option<String>>(8)?,
        "historyMode": row.get::<_, Option<String>>(9)?,
    }))
}

fn assign_project(state: &ProjectState, thread: &mut Value) {
    let assignment = thread["id"]
        .as_str()
        .and_then(|id| state.assignments.get(id));
    let project = match assignment {
        Some(project) => project.as_deref(),
        None => thread["projectId"]
            .as_str()
            .and_then(|id| state.desktop_ids.get(id).map(String::as_str)),
    };
    thread["projectId"] = json!(project);
}

pub fn read_only(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(std::time::Duration::from_secs(2))?;
    Ok(connection)
}
pub fn encode_cursor(value: &Value) -> String {
    URL_SAFE_NO_PAD.encode(value.to_string())
}
pub fn decode_cursor(value: &str) -> Result<Value> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| BridgeError::invalid("分页位置无效，请重新加载。"))?;
    serde_json::from_slice(&bytes).map_err(|_| BridgeError::invalid("分页位置无效，请重新加载。"))
}

impl History {
    pub fn new(codex_home: PathBuf) -> Self {
        Self {
            codex_home,
            mode_cache: Mutex::new(HashMap::new()),
        }
    }
    pub fn state(&self) -> Result<Connection> {
        read_only(&self.codex_home.join("state_5.sqlite"))
    }
    fn history(&self) -> Result<Connection> {
        read_only(&self.codex_home.join("thread_history_1.sqlite"))
    }
    pub fn default_thread_id(&self) -> Result<Option<String>> {
        Ok(self.state()?.query_row("SELECT id FROM threads WHERE archived=0 AND source IN ('vscode','cli') AND (thread_source IN ('user','agent_created_thread') OR thread_source IS NULL) ORDER BY updated_at_ms DESC LIMIT 1",[],|r|r.get(0)).optional()?)
    }
    fn saved_project_settings(&self) -> Result<Value> {
        let path = self.codex_home.join(".codex-global-state.sqlite");
        if !path.try_exists()? {
            return Ok(
                read_json_optional(&self.codex_home.join(".codex-global-state.json"))?
                    .unwrap_or(json!({})),
            );
        }
        // 新版桌面将最新设置写入 SQLite，旧 JSON 可能还留在原处。
        let connection = read_only(&path)?;
        let mut statement = connection.prepare(
            "SELECT key,value FROM settings WHERE scope='global' AND key IN (
                'local-projects','project-order','thread-project-assignments',
                'thread-project-membership-host-ids','projectless-thread-ids',
                'app-server-project-id-by-legacy-project-id-by-host','pinned-thread-ids'
            )",
        )?;
        let mut settings = serde_json::Map::new();
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let value: String = row.get(1)?;
            settings.insert(row.get(0)?, serde_json::from_str(&value)?);
        }
        Ok(Value::Object(settings))
    }
    fn project_state(&self) -> Result<ProjectState> {
        let saved = self.saved_project_settings()?;
        let mut projects = IndexMap::new();
        for id in saved["project-order"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .chain(
                saved["local-projects"]
                    .as_object()
                    .into_iter()
                    .flat_map(|projects| projects.keys().map(String::as_str)),
            )
        {
            let project = &saved["local-projects"][id];
            if let (Some(label), Some(path)) =
                (project["name"].as_str(), project["rootPaths"][0].as_str())
            {
                projects.insert(id.to_owned(), json!({"projectId":id,"label":label,"path":path,"projectKind":"local","hostId":"local"}));
            }
        }
        let host_key = format!("local:{}", self.codex_home.display());
        let server_ids: HashMap<String, String> = saved
            ["app-server-project-id-by-legacy-project-id-by-host"][host_key]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(id, _)| projects.contains_key(*id))
            .filter_map(|(id, server_id)| {
                server_id
                    .as_str()
                    .map(|server_id| (id.clone(), server_id.into()))
            })
            .collect();
        let desktop_ids: HashMap<String, String> = server_ids
            .iter()
            .map(|(id, server_id)| (server_id.clone(), id.clone()))
            .collect();
        let hosts = &saved["thread-project-membership-host-ids"];
        let mut assignments = HashMap::new();
        for (id, membership) in saved["thread-project-assignments"]
            .as_object()
            .into_iter()
            .flatten()
        {
            let project = membership["projectId"]
                .as_str()
                .filter(|_| membership["projectKind"] == "local")
                .map(|id| desktop_ids.get(id).map(String::as_str).unwrap_or(id))
                .filter(|id| projects.contains_key(*id));
            assignments.insert(id.clone(), project.map(str::to_owned));
        }
        for (id, host) in hosts.as_object().into_iter().flatten() {
            if host != "local" {
                assignments.insert(id.clone(), None);
            }
        }
        for id in saved["projectless-thread-ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            assignments.insert(id.into(), None);
        }
        Ok(ProjectState {
            projects,
            assignments,
            server_ids,
            desktop_ids,
            pins: saved["pinned-thread-ids"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        })
    }
    pub fn projects(&self) -> Result<Vec<Value>> {
        Ok(self.project_state()?.projects.into_values().collect())
    }
    pub(crate) fn rollout_path(&self, id: &str) -> Result<Option<PathBuf>> {
        let path: Option<Option<String>> = self
            .state()?
            .query_row("SELECT rollout_path FROM threads WHERE id=?", [id], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(path
            .flatten()
            .filter(|path| !path.is_empty())
            .map(PathBuf::from))
    }
    pub fn metadata(&self, id: &str, include_mode: bool) -> Result<Value> {
        let mut value=self.state()?.query_row("SELECT id,name,title,cwd,model,reasoning_effort,rollout_path,project_id,source,history_mode,COALESCE(archived,0) FROM threads WHERE id=?",[id],|row|{
            let name:Option<String>=row.get(1)?;let title:Option<String>=row.get(2)?;
            Ok(json!({"id":row.get::<_,String>(0)?,"title":name.filter(|s|!s.is_empty()).or(title),"cwd":row.get::<_,String>(3)?,"model":row.get::<_,Option<String>>(4)?,"reasoningEffort":row.get::<_,Option<String>>(5)?,"rolloutPath":row.get::<_,Option<String>>(6)?,"projectId":row.get::<_,Option<String>>(7)?,"source":row.get::<_,Option<String>>(8)?,"historyMode":row.get::<_,Option<String>>(9)?,"archived":row.get::<_,bool>(10)?}))
        }).optional()?.ok_or_else(||BridgeError::new(404,"找不到这条 Codex 聊天。"))?;
        let project = self.project_state()?;
        assign_project(&project, &mut value);
        value["pinnedIndex"] = json!(project
            .pins
            .iter()
            .position(|pin| pin == id)
            .map(|index| index + 1));
        if include_mode {
            value["mode"] = json!(value["rolloutPath"]
                .as_str()
                .and_then(|path| self.read_mode(path)));
        }
        value.as_object_mut().unwrap().shift_remove("rolloutPath");
        Ok(value)
    }
    fn read_mode(&self, path: &str) -> Option<String> {
        let stat = fs::metadata(path).ok()?;
        let time = stat.modified().ok()?;
        if let Some(cached) = self.mode_cache.lock().unwrap().get(path) {
            if cached.size == stat.len() && cached.modified == time {
                return cached.mode.clone();
            }
        }
        let mut file = fs::File::open(path).ok()?;
        let mut position = stat.len();
        let mut tail = vec![];
        let mut found = None;
        while position > 0 {
            let length = position.min(64 * 1024) as usize;
            position -= length as u64;
            file.seek(SeekFrom::Start(position)).ok()?;
            let mut bytes = vec![0; length];
            file.read_exact(&mut bytes).ok()?;
            bytes.extend_from_slice(&tail);
            let boundary = if position == 0 {
                0
            } else {
                bytes
                    .iter()
                    .position(|b| *b == b'\n')
                    .map(|n| n + 1)
                    .unwrap_or(bytes.len())
            };
            for line in bytes[boundary..].split(|b| *b == b'\n').rev() {
                if !std::str::from_utf8(line).is_ok_and(|line| line.contains("\"turn_context\"")) {
                    continue;
                }
                if let Ok(record) = serde_json::from_slice::<Value>(line) {
                    if record["type"] == "turn_context" {
                        if let Some(mode) = record["payload"]["collaboration_mode"]["mode"].as_str()
                        {
                            found = Some(mode.to_string());
                            break;
                        }
                    }
                }
            }
            if found.is_some() {
                break;
            }
            tail = bytes[..boundary.saturating_sub(1)].to_vec();
        }
        self.mode_cache.lock().unwrap().insert(
            path.into(),
            CachedMode {
                size: stat.len(),
                modified: time,
                mode: found.clone(),
            },
        );
        found
    }
    pub fn catalog(
        &self,
        project_id: Option<&str>,
        query: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Value> {
        self.catalog_page(
            &self.project_state()?,
            project_id,
            query,
            cursor,
            limit,
            false,
        )
    }
    pub fn archived_catalog(
        &self,
        query: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Value> {
        self.catalog_page(&self.project_state()?, None, query, cursor, limit, true)
    }
    fn catalog_page(
        &self,
        state: &ProjectState,
        project_id: Option<&str>,
        query: &str,
        cursor: Option<&str>,
        limit: usize,
        archived: bool,
    ) -> Result<Value> {
        let mut clauses = vec![
            if archived { "archived=1" } else { "archived=0" },
            "source IN ('vscode','cli')",
            "(thread_source IN ('user','agent_created_thread') OR thread_source IS NULL)",
        ];
        let mut params: Vec<SqlValue> = vec![];
        let mut project_path = "";
        if let Some(id) = project_id {
            let project = state
                .projects
                .get(id)
                .ok_or_else(|| BridgeError::new(404, "该项目已不可用，请返回项目列表。"))?;
            project_path = project["path"].as_str().unwrap_or_default();
            // 显式的桌面归属优先。数据库里的项目 ID 只使用桌面已保存的对应关系。
            clauses.push("(id IN (SELECT value FROM json_each(?)) OR (id NOT IN (SELECT value FROM json_each(?)) AND project_id=?))");
            let assigned = state
                .assignments
                .iter()
                .filter(|(_, project)| project.as_deref() == Some(id))
                .map(|(thread, _)| thread)
                .collect::<Vec<_>>();
            let known = state.assignments.keys().collect::<Vec<_>>();
            params.extend([
                json!(assigned).to_string().into(),
                json!(known).to_string().into(),
                state
                    .server_ids
                    .get(id)
                    .cloned()
                    .map_or(SqlValue::Null, SqlValue::Text),
            ]);
        }
        let mut scope = json!([project_id.unwrap_or(""), project_path, query]).to_string();
        if archived {
            scope.push_str(":archived");
        }
        if !query.is_empty() {
            clauses.push("COALESCE(NULLIF(name,''),title,'') LIKE ? ESCAPE '\\'");
            params.push(
                format!(
                    "%{}%",
                    query
                        .replace('\\', r"\\")
                        .replace('%', r"\%")
                        .replace('_', r"\_")
                )
                .into(),
            );
        }
        if let Some(cursor) = cursor {
            let page = decode_cursor(cursor)?;
            if page["scope"] != scope
                || !page["time"].is_i64()
                || page["id"].as_str().is_none_or(|s| s.is_empty())
            {
                return Err(BridgeError::invalid("聊天分页位置无效，请返回第一页。"));
            }
            clauses.push("(recency_at_ms < ? OR (recency_at_ms = ? AND id < ?))");
            params.extend([
                page["time"].as_i64().unwrap().into(),
                page["time"].as_i64().unwrap().into(),
                page["id"].as_str().unwrap().to_string().into(),
            ]);
        }
        params.push(((limit + 1) as i64).into());
        let connection = self.state()?;
        let mut statement=connection.prepare(&format!("SELECT id,name,title,cwd,model,reasoning_effort,project_id,recency_at_ms,source,history_mode FROM threads WHERE {} ORDER BY recency_at_ms DESC,id DESC LIMIT ?",clauses.join(" AND ")))?;
        let mut rows = statement
            .query_map(params_from_iter(params), catalog_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let more = rows.len() > limit;
        rows.truncate(limit);
        for row in &mut rows {
            assign_project(state, row);
            row["archived"] = json!(archived);
        }
        let next = rows
            .last()
            .filter(|_| more)
            .map(|r| encode_cursor(&json!({"scope":scope,"time":r["updatedAt"],"id":r["id"]})));
        Ok(json!({"threads":rows,"nextCursor":next}))
    }
    pub fn thread_list(&self, cursor: Option<&str>) -> Result<Value> {
        let state = self.project_state()?;
        let page = self.catalog_page(&state, None, "", cursor, 50, false)?;
        let mut entries = IndexMap::<String, Value>::new();
        if cursor.is_none() && !state.pins.is_empty() {
            let pins = &state.pins;
            let connection = self.state()?;
            let mut statement = connection.prepare(
                    "SELECT id,name,title,cwd,model,reasoning_effort,project_id,recency_at_ms,source,history_mode FROM threads WHERE archived=0 AND source IN ('vscode','cli') AND (thread_source IN ('user','agent_created_thread') OR thread_source IS NULL) AND id IN (SELECT value FROM json_each(?))",
                )?;
            let mut threads = statement
                .query_map([json!(pins).to_string()], catalog_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            threads.sort_by_key(|thread| pins.iter().position(|id| *id == thread["id"]));
            for mut thread in threads {
                assign_project(&state, &mut thread);
                thread["pinnedIndex"] =
                    json!(pins.iter().position(|id| *id == thread["id"]).unwrap() + 1);
                entries.insert(thread["id"].as_str().unwrap().into(), thread);
            }
        }
        for thread in page["threads"].as_array().unwrap() {
            entries
                .entry(thread["id"].as_str().unwrap().into())
                .or_insert_with(|| thread.clone());
        }
        if !entries.is_empty() {
            let connection = self.history()?;
            let mut statement = connection.prepare(
                "SELECT thread_id,status FROM thread_turns t WHERE thread_id IN (SELECT value FROM json_each(?)) AND rollout_ordinal=(SELECT MAX(rollout_ordinal) FROM thread_turns WHERE thread_id=t.thread_id)",
            )?;
            let rows = statement.query_map(
                [json!(entries.keys().collect::<Vec<_>>()).to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            for row in rows {
                let (id, status) = row?;
                if let Some(thread) = entries.get_mut(&id) {
                    thread["status"] = json!(if status == "inProgress" {
                        "active"
                    } else {
                        "idle"
                    });
                }
            }
        }
        Ok(
            json!({"threads":entries.into_values().collect::<Vec<_>>(),"nextCursor":page["nextCursor"]}),
        )
    }
    pub fn page(&self, id: &str, cursor: Option<&str>, limit: usize) -> Result<Value> {
        self.page_with_metadata(id, cursor, limit, None)
    }
    pub(crate) fn page_with_metadata(
        &self,
        id: &str,
        cursor: Option<&str>,
        limit: usize,
        metadata: Option<Value>,
    ) -> Result<Value> {
        let ordinal = if let Some(cursor) = cursor {
            let page = decode_cursor(cursor)?;
            if page["threadId"] != id || page["ordinal"].as_i64().is_none_or(|n| n < 0) {
                return Err(BridgeError::invalid("消息分页位置无效，请刷新聊天。"));
            }
            page["ordinal"].as_i64()
        } else {
            None
        };
        let connection = self.history()?;
        let mut params: Vec<SqlValue> = vec![id.to_string().into()];
        if let Some(n) = ordinal {
            params.push(n.into());
        }
        params.push(((limit + 1) as i64).into());
        let mut statement=connection.prepare(&format!("SELECT turn_id,status,error_json,started_at,completed_at,rollout_ordinal FROM thread_turns WHERE thread_id=? {} ORDER BY rollout_ordinal DESC LIMIT ?",if ordinal.is_some(){"AND rollout_ordinal < ?"}else{""}))?;
        let mut turns=statement.query_map(params_from_iter(params),|r|{
            let error:Option<String>=r.get(2)?;
            Ok(json!({"id":r.get::<_,String>(0)?,"status":r.get::<_,String>(1)?,"error":error.and_then(|s|serde_json::from_str::<Value>(&s).ok()),"startedAt":r.get::<_,Option<i64>>(3)?,"completedAt":r.get::<_,Option<i64>>(4)?,"ordinal":r.get::<_,i64>(5)?}))
        })?.collect::<std::result::Result<Vec<_>,_>>()?;
        let more = turns.len() > limit;
        turns.truncate(limit);
        let next = turns
            .last()
            .filter(|_| more)
            .map(|r| encode_cursor(&json!({"threadId":id,"ordinal":r["ordinal"]})));
        let fields = SUMMARY_FIELDS
            .iter()
            .map(|f| format!("'$.{f}'"))
            .chain(std::iter::once("'$.arguments.title'".into()))
            .collect::<Vec<_>>()
            .join(",");
        let mut items=connection.prepare(&format!("SELECT updated_at_ordinal,CASE WHEN item_type IN ('userMessage','agentMessage','plan') OR (item_type='functionCallOutput' AND json_extract(item_json,'$.name') IN ('create_thread','send_message_to_thread') AND json_extract(item_json,'$.namespace')='codex_app') THEN item_json WHEN item_type='reasoning' THEN json_object('id',item_id,'type',item_type,'summary',json_extract(item_json,'$.summary')) ELSE json_extract(item_json,{fields}) END FROM thread_items WHERE thread_id=? AND turn_id=? ORDER BY rollout_ordinal ASC"))?;
        for turn in &mut turns {
            let rows = items
                .query_map(params![id, turn["id"].as_str().unwrap()], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut values = vec![];
            for (ordinal, text) in rows {
                let value: Value = serde_json::from_str(&text)?;
                let value = if let Some(fields) = value.as_array() {
                    let mut result = Value::Object(
                        SUMMARY_FIELDS
                            .iter()
                            .zip(fields)
                            .filter(|(_, v)| !v.is_null())
                            .map(|(k, v)| (k.to_string(), v.clone()))
                            .collect(),
                    );
                    if let Some(title) = fields.last().filter(|v| !v.is_null()) {
                        result["arguments"] = json!({"title":title});
                    }
                    result["detailVersion"] = json!(format!("history:{ordinal}"));
                    result
                } else {
                    value
                };
                if value["type"] != "reasoning"
                    || value["summary"].as_array().is_some_and(|s| !s.is_empty())
                {
                    values.push(summarize_item(&value));
                }
            }
            turn["items"] = json!(values);
            turn.as_object_mut().unwrap().shift_remove("ordinal");
        }
        turns.reverse();
        let thread = match metadata {
            Some(value) => value,
            None => self.metadata(id, true)?,
        };
        Ok(json!({"thread":thread,"turns":turns,"nextCursor":next}))
    }
    pub fn changed_files(&self, id: &str) -> Result<HashSet<String>> {
        let connection = self.history()?;
        let mut statement = connection.prepare(
            "SELECT DISTINCT change.value ->> '$.path'
             FROM thread_items AS item, json_each(item.item_json, '$.changes') AS change
             WHERE item.thread_id=? AND item.item_type='fileChange'
               AND json_extract(item.item_json, '$.status')='completed'
               AND change.type='object' AND json_type(change.value, '$.path')='text'
               AND (change.value ->> '$.path') <> ''",
        )?;
        let paths = statement
            .query_map([id], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(paths)
    }
    pub fn item(&self, thread_id: &str, item_id: &str) -> Result<Option<Value>> {
        let text: Option<String> = self
            .history()?
            .query_row(
                "SELECT item_json FROM thread_items WHERE thread_id=? AND item_id=?",
                params![thread_id, item_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(text.map(|t| serde_json::from_str(&t)).transpose()?)
    }
    pub fn recent_models(&self) -> Result<Vec<String>> {
        let connection = self.state()?;
        let mut statement=connection.prepare("SELECT DISTINCT model FROM threads WHERE model IS NOT NULL AND model!='' ORDER BY model")?;
        let values = statement
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(values)
    }
    pub fn notification_turns(&self, since: i64) -> Result<Vec<Value>> {
        let history = self.history()?;
        let state = self.state()?;
        let mut statement=history.prepare("SELECT thread_id,turn_id,status,error_json,completed_at FROM thread_turns WHERE status IN ('completed','failed') AND completed_at>=? ORDER BY completed_at,thread_id,rollout_ordinal")?;
        let rows = statement
            .query_map([since], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut values = vec![];
        for (thread, id, status, error, completed) in rows {
            let title: Option<String> = state
                .query_row(
                    "SELECT COALESCE(NULLIF(name,''),title,'') FROM threads WHERE id=?",
                    [&thread],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(title) = title {
                values.push(json!({"threadId":thread,"title":title,"id":id,"status":status,"error":error.and_then(|s|serde_json::from_str::<Value>(&s).ok()),"completedAt":completed}));
            }
        }
        Ok(values)
    }
}
