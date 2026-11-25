use crate::{
    history::{decode_cursor, encode_cursor, read_only},
    paths::normalized,
    BridgeError, Result,
};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Instant, SystemTime},
};

const FIELDS: [&str; 6] = [
    "totalTokens",
    "inputTokens",
    "outputTokens",
    "cachedInputTokens",
    "cacheWriteInputTokens",
    "reasoningOutputTokens",
];
const WIRE_FIELDS: [&str; 6] = [
    "total_tokens",
    "input_tokens",
    "output_tokens",
    "cached_input_tokens",
    "cache_write_input_tokens",
    "reasoning_output_tokens",
];

fn metrics(value: &Value) -> Option<Value> {
    let count = |v: &Value| {
        v.as_i64()
            .filter(|n| *n >= 0 && *n <= 9_007_199_254_740_991)
    };
    count(&value["total_tokens"])?;
    Some(Value::Object(
        FIELDS
            .iter()
            .zip(WIRE_FIELDS)
            .map(|(key, wire)| (key.to_string(), json!(count(&value[wire]))))
            .collect(),
    ))
}
fn empty_metrics() -> Value {
    Value::Object(
        FIELDS
            .iter()
            .map(|key| (key.to_string(), json!(0)))
            .collect(),
    )
}
fn aggregate<'a>(events: impl Iterator<Item = &'a UsageEvent>) -> Value {
    let mut result = empty_metrics();
    for event in events {
        for key in FIELDS {
            result[key] = match (result[key].as_i64(), event.metrics[key].as_i64()) {
                (Some(a), Some(b)) => json!(a + b),
                _ => Value::Null,
            };
        }
    }
    result
}
fn agrees(a: &Value, b: &Value) -> bool {
    a["totalTokens"] == b["totalTokens"]
        && FIELDS
            .iter()
            .all(|key| a[*key].is_null() || b[*key].is_null() || a[*key] == b[*key])
}
fn identity(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.encode_utf16().count() <= 256)
        .map(str::to_string)
}
fn stamp(value: &Value) -> Option<String> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|date| {
            date.with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true)
        })
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageState {
    pub thread_id: Option<String>,
    pub created_at: Option<String>,
    pub parent_thread_id: Option<String>,
    pub inherited_thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub model: Option<String>,
    pub previous: Option<Value>,
    pub gap_events: u64,
    pub unassigned_events: u64,
    pub malformed_lines: u64,
    pub token_records: u64,
    pub unsupported_records: u64,
    pub history_base: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEvent {
    pub fingerprint: String,
    pub thread_id: Option<String>,
    #[serde(default)]
    pub owner_quality: u8,
    #[serde(default)]
    pub turn_id: Option<String>,
    pub timestamp: String,
    pub model: Option<String>,
    pub metrics: Value,
}

pub fn parse_usage_line(line: &str, state: &mut UsageState) -> Option<UsageEvent> {
    let record: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => {
            state.malformed_lines += 1;
            return None;
        }
    };
    let p = &record["payload"];
    if !p.is_object() {
        return None;
    }
    if record["type"] == "session_meta" {
        let id = identity(&p["id"])?;
        if state.thread_id.is_none() {
            state.thread_id = Some(id);
            state.created_at = stamp(&p["timestamp"]).or_else(|| stamp(&record["timestamp"]));
            state.parent_thread_id =
                identity(&p["source"]["subagent"]["thread_spawn"]["parent_thread_id"])
                    .or_else(|| identity(&p["forked_from_id"]))
                    .or_else(|| identity(&p["history_base"]["thread_id"]));
            state.history_base = !p["history_base"].is_null() && p["history_base"] != false;
        } else if state.thread_id.as_ref() != Some(&id) {
            state.inherited_thread_id = Some(id);
        }
        return None;
    }
    if record["type"] == "turn_context" {
        state.turn_id = identity(&p["turn_id"]);
        state.model = p["model"]
            .as_str()
            .filter(|s| s.encode_utf16().count() <= 1000)
            .map(str::to_string);
        return None;
    }
    if record["type"] != "event_msg" || p["type"] != "token_count" || p["info"].is_null() {
        return None;
    }
    let info = &p["info"];
    state.token_records += 1;
    let (Some(total), Some(last)) = (
        metrics(&info["total_token_usage"]),
        metrics(&info["last_token_usage"]),
    ) else {
        state.unsupported_records += 1;
        return None;
    };
    let previous = state.previous.replace(total.clone());
    if previous.as_ref() == Some(&total) {
        return None;
    }
    if let Some(previous) = previous {
        let delta = Value::Object(
            FIELDS
                .iter()
                .map(|key| {
                    (
                        key.to_string(),
                        match (total[*key].as_i64(), previous[*key].as_i64()) {
                            (Some(a), Some(b)) => json!(a - b),
                            _ => Value::Null,
                        },
                    )
                })
                .collect(),
        );
        if FIELDS
            .iter()
            .any(|key| delta[*key].as_i64().is_some_and(|n| n < 0))
            || !agrees(&delta, &last)
        {
            state.gap_events += 1;
            return None;
        }
    } else if !agrees(&total, &last) {
        state.gap_events += 1;
        return None;
    }
    if last["totalTokens"] == 0 {
        return None;
    }
    let timestamp = stamp(&record["timestamp"]);
    if timestamp.is_none() || state.thread_id.is_none() || state.turn_id.is_none() {
        state.unassigned_events += 1;
        return None;
    }
    let timestamp = timestamp.unwrap();
    let mut owner = state.thread_id.clone();
    let mut quality = 2;
    if state
        .created_at
        .as_ref()
        .is_some_and(|created| timestamp < *created)
    {
        owner = state
            .inherited_thread_id
            .clone()
            .or(state.parent_thread_id.clone());
        quality = 1;
    }
    let mut metrics = last.clone();
    if info["last_usage_includes_reasoning"] == false {
        metrics["reasoningOutputTokens"] = Value::Null;
        state.gap_events += 1;
    }
    Some(UsageEvent {
        fingerprint: crate::digest(json!([state.turn_id, timestamp, total, last]).to_string()),
        thread_id: owner,
        owner_quality: quality,
        turn_id: state.turn_id.clone(),
        timestamp,
        model: state.model.clone(),
        metrics,
    })
}

#[derive(Clone)]
struct Source {
    thread_id: Option<String>,
    status: String,
    state: UsageState,
}
#[derive(Clone)]
struct UsageThread {
    title: String,
    parent: Option<String>,
    tokens: i64,
}
struct Snapshot {
    generated: String,
    events: Vec<UsageEvent>,
    sources: Vec<Source>,
    threads: HashMap<String, UsageThread>,
    index_unavailable: bool,
    discovery_errors: u64,
}
#[derive(Default)]
struct Status {
    snapshot: Option<Arc<Snapshot>>,
    refreshing: bool,
    last_scan: Option<Instant>,
    fatal: Option<String>,
    discovered: usize,
    processed: usize,
}
pub struct Usage {
    home: PathBuf,
    database: PathBuf,
    status: Mutex<Status>,
    closed: AtomicBool,
    done: tokio::sync::Notify,
}

#[derive(Clone)]
pub struct UsageQuery {
    pub period: String,
    pub time_zone: String,
    today: String,
    start: Option<String>,
    zone: chrono_tz::Tz,
}
impl UsageQuery {
    pub fn parse(period: &str, time_zone: &str, now: DateTime<Utc>) -> Result<Self> {
        if !["today", "7d", "30d", "all"].contains(&period) {
            return Err(BridgeError::invalid("时间范围无效。"));
        }
        let zone = time_zone
            .parse::<chrono_tz::Tz>()
            .map_err(|_| BridgeError::invalid("时区无效。"))?;
        let date = now.with_timezone(&zone).date_naive();
        let today = date.to_string();
        let start = (period != "all").then(|| {
            (date
                - Duration::days(match period {
                    "7d" => 6,
                    "30d" => 29,
                    _ => 0,
                }))
            .to_string()
        });
        Ok(Self {
            period: period.into(),
            time_zone: time_zone.into(),
            today,
            start,
            zone,
        })
    }
    fn day(&self, event: &UsageEvent) -> String {
        DateTime::parse_from_rfc3339(&event.timestamp)
            .map(|d| d.with_timezone(&self.zone).date_naive().to_string())
            .unwrap_or_default()
    }
    fn selected(&self, event: &UsageEvent) -> bool {
        let day = self.day(event);
        self.start.as_ref().is_none_or(|start| day >= *start) && day <= self.today
    }
}

fn meta(db: &Connection, key: &str) -> Result<Value> {
    let value: Option<String> = db
        .query_row("SELECT value FROM usage_meta WHERE key=?", [key], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(value
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null))
}
fn save_meta(db: &Connection, key: &str, value: Value) -> Result<()> {
    db.execute(
        "INSERT OR REPLACE INTO usage_meta VALUES (?,?)",
        params![key, value.to_string()],
    )?;
    Ok(())
}
fn load_snapshot(db: &Connection) -> Result<Snapshot> {
    let mut statement =
        db.prepare("SELECT fingerprint,thread_id,timestamp,model,metrics FROM usage_events")?;
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut events = vec![];
    for (fingerprint, thread_id, timestamp, model, metrics) in rows {
        events.push(UsageEvent {
            fingerprint,
            thread_id,
            timestamp,
            model,
            metrics: serde_json::from_str(&metrics)?,
            owner_quality: 0,
            turn_id: None,
        });
    }
    let mut statement = db.prepare("SELECT thread_id,status,state FROM usage_sources")?;
    let sources = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .map(|r| {
            let (id, status, state) = r?;
            Ok(Source {
                thread_id: id,
                status,
                state: serde_json::from_str(&state)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut statement =
        db.prepare("SELECT thread_id,title,parent_thread_id,tokens_used FROM usage_threads")?;
    let threads = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                UsageThread {
                    title: r.get(1)?,
                    parent: r.get(2)?,
                    tokens: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                },
            ))
        })?
        .collect::<std::result::Result<HashMap<_, _>, _>>()?;
    Ok(Snapshot {
        generated: meta(db, "generatedAt")?
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)),
        events,
        sources,
        threads,
        index_unavailable: meta(db, "indexUnavailable")? == true,
        discovery_errors: meta(db, "discoveryErrors")?.as_u64().unwrap_or(0),
    })
}
impl Usage {
    pub fn open(home: PathBuf, directory: PathBuf) -> Result<Arc<Self>> {
        fs::create_dir_all(&directory)?;
        let database = directory.join("usage.sqlite");
        let db = Connection::open(&database)?;
        db.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS usage_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS usage_threads(thread_id TEXT PRIMARY KEY,title TEXT NOT NULL,parent_thread_id TEXT,tokens_used INTEGER);
            CREATE TABLE IF NOT EXISTS usage_sources(path TEXT PRIMARY KEY,thread_id TEXT,expected_thread_id TEXT,identity TEXT,size INTEGER NOT NULL DEFAULT 0,mtime REAL NOT NULL DEFAULT 0,offset INTEGER NOT NULL DEFAULT 0,prefix_length INTEGER NOT NULL DEFAULT 0,prefix_hash TEXT,boundary_hash TEXT,state TEXT NOT NULL,status TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS usage_contributions(source TEXT NOT NULL,fingerprint TEXT NOT NULL,owner TEXT,quality INTEGER NOT NULL,event TEXT NOT NULL,PRIMARY KEY(source,fingerprint));
            CREATE INDEX IF NOT EXISTS usage_contribution_event ON usage_contributions(fingerprint);
            CREATE TABLE IF NOT EXISTS usage_events(fingerprint TEXT PRIMARY KEY,thread_id TEXT,timestamp TEXT NOT NULL,model TEXT,metrics TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS usage_event_thread ON usage_events(thread_id,timestamp);")?;
        if meta(&db, "parserVersion")? != 1 {
            db.execute_batch("DELETE FROM usage_sources;DELETE FROM usage_contributions;DELETE FROM usage_events;DELETE FROM usage_threads;DELETE FROM usage_meta;")?;
            save_meta(&db, "parserVersion", json!(1))?;
        }
        let snapshot = if meta(&db, "complete")? == true {
            Some(Arc::new(load_snapshot(&db)?))
        } else {
            None
        };
        let fatal = meta(&db, "fatalError")?.as_str().map(str::to_string);
        Ok(Arc::new(Self {
            home,
            database,
            status: Mutex::new(Status {
                snapshot,
                fatal,
                ..Default::default()
            }),
            closed: AtomicBool::new(false),
            done: tokio::sync::Notify::new(),
        }))
    }
    fn start_refresh(self: &Arc<Self>, force: bool) {
        let mut state = self.status.lock().unwrap();
        if state.refreshing
            || self.closed.load(Ordering::Acquire)
            || (!force
                && state
                    .last_scan
                    .is_some_and(|time| time.elapsed().as_secs_f64() < 1.0))
        {
            return;
        }
        state.refreshing = true;
        state.last_scan = Some(Instant::now());
        drop(state);
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            let result = this.scan();
            let mut state = this.status.lock().unwrap();
            match result {
                Ok((snapshot, fatal)) => {
                    state.snapshot = Some(Arc::new(snapshot));
                    state.fatal = fatal;
                }
                Err(error) => state.fatal = Some(error.message),
            }
            state.refreshing = false;
            drop(state);
            this.done.notify_waiters();
        });
    }
    pub async fn refresh(self: &Arc<Self>) -> Result<()> {
        self.start_refresh(true);
        loop {
            let notified = self.done.notified();
            if !self.status.lock().unwrap().refreshing {
                break;
            }
            notified.await;
        }
        self.status
            .lock()
            .unwrap()
            .fatal
            .clone()
            .map_or(Ok(()), |message| Err(BridgeError::unavailable(message)))
    }
    pub async fn close(&self) {
        self.closed.store(true, Ordering::Release);
        loop {
            let notified = self.done.notified();
            if !self.status.lock().unwrap().refreshing {
                break;
            }
            notified.await;
        }
    }
    fn scan(&self) -> Result<(Snapshot, Option<String>)> {
        let mut db = Connection::open(&self.database)?;
        save_meta(&db, "complete", json!(false))?;
        let mut sources = HashMap::<String, Option<String>>::new();
        let mut index_unavailable = false;
        let mut discovery_errors = 0u64;
        let indexed = (|| -> Result<()> {
            let state = read_only(&self.home.join("state_5.sqlite"))?;
            let mut columns = state.prepare("PRAGMA table_info(threads)")?;
            let names = columns
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<std::result::Result<HashSet<_>, _>>()?;
            if !names.contains("id") || !names.contains("rollout_path") {
                return Err(BridgeError::unavailable("用量目录格式暂不支持。"));
            }
            let optional = [
                "name",
                "title",
                "source",
                "tokens_used",
                "attached_thread_parent_id",
            ]
            .iter()
            .map(|s| {
                if names.contains(*s) {
                    s.to_string()
                } else {
                    format!("NULL AS {s}")
                }
            })
            .collect::<Vec<_>>()
            .join(",");
            let mut rows =
                state.prepare(&format!("SELECT id,rollout_path,{optional} FROM threads"))?;
            let records = rows
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let transaction = db.transaction()?;
            for (id, path, name, title, source, tokens, parent) in records {
                let source = source.and_then(|s| serde_json::from_str::<Value>(&s).ok());
                let parent = parent.or_else(|| {
                    source
                        .as_ref()
                        .and_then(|s| s["subagent"]["thread_spawn"]["parent_thread_id"].as_str())
                        .map(str::to_string)
                });
                transaction.execute("INSERT INTO usage_threads VALUES (?,?,?,?) ON CONFLICT(thread_id) DO UPDATE SET title=excluded.title,parent_thread_id=excluded.parent_thread_id,tokens_used=excluded.tokens_used",params![id,name.filter(|s|!s.is_empty()).or(title).unwrap_or_else(||"未命名对话".into()),parent,tokens])?;
                if let Some(path) = path.filter(|s| !s.is_empty()) {
                    sources.insert(normalized(Path::new(&path)), Some(id));
                }
            }
            transaction.commit()?;
            Ok(())
        })();
        if indexed.is_err() {
            index_unavailable = true;
        }
        for root in [
            self.home.join("sessions"),
            self.home.join("archived_sessions"),
        ] {
            if !root.exists() {
                continue;
            }
            for entry in walkdir::WalkDir::new(root) {
                match entry {
                    Ok(entry)
                        if entry.file_type().is_file()
                            && entry.path().extension().is_some_and(|e| e == "jsonl") =>
                    {
                        sources.entry(normalized(entry.path())).or_insert(None);
                    }
                    Err(_) => discovery_errors += 1,
                    _ => {}
                }
            }
        }
        {
            let mut statement = db.prepare("SELECT path,expected_thread_id FROM usage_sources")?;
            for row in statement.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })? {
                let (path, id) = row?;
                sources.entry(path).or_insert(id);
            }
        }
        {
            let mut state = self.status.lock().unwrap();
            state.discovered = sources.len();
            state.processed = 0;
        }
        for (path, expected) in &sources {
            if self.closed.load(Ordering::Acquire) {
                return Err(BridgeError::unavailable("用量索引已停止。"));
            }
            self.read_source(&mut db, Path::new(path), expected.as_deref())?;
            self.status.lock().unwrap().processed += 1;
        }
        save_meta(&db, "indexUnavailable", json!(index_unavailable))?;
        save_meta(&db, "discoveryErrors", json!(discovery_errors))?;
        save_meta(
            &db,
            "generatedAt",
            json!(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)),
        )?;
        let snapshot = load_snapshot(&db)?;
        let fatal = if !snapshot.events.iter().any(|e| e.thread_id.is_some())
            && !snapshot.sources.iter().any(|s| s.status == "ready")
            && (index_unavailable || !sources.is_empty())
        {
            Some(
                if snapshot.sources.iter().any(|s| s.status == "unsupported") {
                    "本机用量记录格式暂不支持，请检查 Codex 版本后重试。"
                } else {
                    "本机用量记录暂时无法读取，请检查电脑上的 Codex 后重试。"
                }
                .into(),
            )
        } else {
            None
        };
        save_meta(&db, "fatalError", json!(fatal))?;
        save_meta(&db, "complete", json!(true))?;
        Ok((snapshot, fatal))
    }
    fn read_source(&self, db: &mut Connection, path: &Path, expected: Option<&str>) -> Result<()> {
        struct Old {
            thread_id: Option<String>,
            identity: Option<String>,
            size: u64,
            mtime: f64,
            offset: u64,
            prefix_length: usize,
            prefix_hash: Option<String>,
            boundary_hash: Option<String>,
            state: String,
            status: String,
        }
        let key = path.to_string_lossy().to_string();
        let old=db.query_row("SELECT thread_id,identity,size,mtime,offset,prefix_length,prefix_hash,boundary_hash,state,status FROM usage_sources WHERE path=?",[&key],|r|Ok(Old{thread_id:r.get(0)?,identity:r.get(1)?,size:r.get(2)?,mtime:r.get(3)?,offset:r.get(4)?,prefix_length:r.get(5)?,prefix_hash:r.get(6)?,boundary_hash:r.get(7)?,state:r.get(8)?,status:r.get(9)?})).optional()?;
        let mut file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) => {
                let state = old
                    .as_ref()
                    .map(|o| o.state.clone())
                    .unwrap_or_else(|| serde_json::to_string(&UsageState::default()).unwrap());
                let status = if error.kind() == std::io::ErrorKind::NotFound {
                    "missing"
                } else {
                    "unreadable"
                };
                db.execute("INSERT INTO usage_sources(path,thread_id,expected_thread_id,state,status) VALUES (?,?,?,?,?) ON CONFLICT(path) DO UPDATE SET status=excluded.status,expected_thread_id=COALESCE(excluded.expected_thread_id,usage_sources.expected_thread_id)",params![key,old.as_ref().and_then(|o|o.thread_id.as_deref()).or(expected),expected,state,status])?;
                return Ok(());
            }
        };
        let stat = file.metadata()?;
        let modified = stat
            .modified()?
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
            * 1000.0;
        let created = stat
            .created()
            .ok()
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|time| time.as_nanos())
            .unwrap_or(0);
        #[cfg(unix)]
        let file_id = {
            use std::os::unix::fs::MetadataExt;
            format!("{}:{}", stat.dev(), stat.ino())
        };
        #[cfg(windows)]
        let file_id = key.clone();
        let identity = format!("rust:{file_id}:{created}");
        if let Some(old) = &old {
            if stat.len() == old.size
                && modified == old.mtime
                && old.identity.as_deref() == Some(&identity)
                && expected.is_none_or(|e| Some(e) == old.thread_id.as_deref())
            {
                if old.status == "missing" || old.status == "unreadable" {
                    let state: UsageState = serde_json::from_str(&old.state)?;
                    db.execute(
                        "UPDATE usage_sources SET status=? WHERE path=?",
                        params![
                            if state.thread_id.is_none()
                                || (state.token_records > 0
                                    && state.token_records == state.unsupported_records)
                            {
                                "unsupported"
                            } else {
                                "ready"
                            },
                            key
                        ],
                    )?;
                }
                return Ok(());
            }
        }
        let mut append = old.as_ref().is_some_and(|old| {
            old.identity.as_deref() == Some(&identity)
                && stat.len() > old.size
                && old.offset <= stat.len()
        });
        if append {
            let old = old.as_ref().unwrap();
            let (prefix, boundary) = checkpoint(&mut file, old.offset, old.prefix_length)?;
            append = Some(prefix) == old.prefix_hash && Some(boundary) == old.boundary_hash;
        }
        let mut state = if append {
            serde_json::from_str(&old.as_ref().unwrap().state)?
        } else {
            UsageState::default()
        };
        let mut offset = if append {
            old.as_ref().unwrap().offset
        } else {
            0
        };
        file.seek(SeekFrom::Start(offset))?;
        let mut reader = BufReader::new(file);
        let mut bytes = vec![];
        let mut events = vec![];
        loop {
            bytes.clear();
            if reader.read_until(b'\n', &mut bytes)? == 0 {
                break;
            }
            if !bytes.ends_with(b"\n") {
                break;
            }
            offset += bytes.len() as u64;
            let line = String::from_utf8_lossy(&bytes);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line.contains("\"session_meta\"")
                || line.contains("\"turn_context\"")
                || line.contains("\"token_count\"")
            {
                if let Some(event) = parse_usage_line(line, &mut state) {
                    events.push(event);
                }
            } else if !line.starts_with('{') || !line.ends_with('}') {
                state.malformed_lines += 1;
            }
        }
        let mut file = reader.into_inner();
        let prefix_length = offset.min(256) as usize;
        let (prefix_hash, boundary_hash) = checkpoint(&mut file, offset, prefix_length)?;
        let mismatch = expected.is_some()
            && state.thread_id.is_some()
            && expected != state.thread_id.as_deref();
        let status = if state.thread_id.is_none()
            || mismatch
            || (state.token_records > 0 && state.token_records == state.unsupported_records)
        {
            "unsupported"
        } else {
            "ready"
        };
        let mut fingerprints: HashSet<String> =
            events.iter().map(|e| e.fingerprint.clone()).collect();
        let transaction = db.transaction()?;
        if !append || mismatch {
            let mut statement = transaction
                .prepare("SELECT fingerprint FROM usage_contributions WHERE source=?")?;
            fingerprints.extend(
                statement
                    .query_map([&key], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?,
            );
            drop(statement);
            transaction.execute("DELETE FROM usage_contributions WHERE source=?", [&key])?;
        }
        if !mismatch {
            for event in &events {
                transaction.execute(
                    "INSERT OR REPLACE INTO usage_contributions VALUES (?,?,?,?,?)",
                    params![
                        key,
                        event.fingerprint,
                        event.thread_id,
                        event.owner_quality,
                        serde_json::to_string(event)?
                    ],
                )?;
                if let Some(id) = &event.thread_id {
                    transaction.execute(
                        "INSERT OR IGNORE INTO usage_threads VALUES (?,'未命名对话',NULL,NULL)",
                        [id],
                    )?;
                }
            }
            if let Some(id) = &state.thread_id {
                transaction.execute(
                    "INSERT OR IGNORE INTO usage_threads VALUES (?,'未命名对话',?,NULL)",
                    params![id, state.parent_thread_id],
                )?;
            }
        }
        transaction.execute(
            "INSERT OR REPLACE INTO usage_sources VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                key,
                if mismatch {
                    expected
                } else {
                    state.thread_id.as_deref().or(expected)
                },
                expected,
                identity,
                stat.len(),
                modified,
                offset,
                prefix_length,
                prefix_hash,
                boundary_hash,
                serde_json::to_string(&state)?,
                status
            ],
        )?;
        for fingerprint in fingerprints {
            reconcile(&transaction, &fingerprint)?;
        }
        transaction.commit()?;
        Ok(())
    }
    fn query_state(
        self: &Arc<Self>,
        query: &UsageQuery,
        thread: Option<&str>,
    ) -> Result<(Value, Option<Arc<Snapshot>>)> {
        if self.closed.load(Ordering::Acquire) {
            return Err(BridgeError::unavailable("用量服务已关闭。"));
        }
        self.start_refresh(false);
        let state = self.status.lock().unwrap();
        let snapshot = state.snapshot.clone();
        if let Some(error) = &state.fatal {
            return Err(BridgeError::unavailable(error));
        }
        if snapshot
            .as_ref()
            .is_some_and(|s| thread.is_some_and(|id| !s.threads.contains_key(id)))
        {
            return Err(BridgeError::new(404, "找不到这条对话的用量记录。"));
        }
        let coverage = coverage(
            snapshot.as_deref(),
            thread,
            state.discovered,
            state.processed,
        );
        let partial = [
            "missingSources",
            "unreadableSources",
            "unsupportedSources",
            "unassignedEvents",
            "gapEvents",
            "unavailableThreads",
        ]
        .iter()
        .any(|key| coverage[*key].as_u64().unwrap_or(0) > 0)
            || coverage["indexUnavailable"] == true;
        Ok((
            json!({"status":if snapshot.is_none(){"indexing"}else if partial{"partial"}else{"ready"},"generatedAt":snapshot.as_ref().map(|s|s.generated.clone()).unwrap_or_else(||Utc::now().to_rfc3339_opts(SecondsFormat::Millis,true)),"period":query.period,"timeZone":query.time_zone,"coverage":coverage,"refreshing":state.refreshing}),
            snapshot,
        ))
    }
    pub fn overview(self: &Arc<Self>, query: &UsageQuery, thread: Option<&str>) -> Result<Value> {
        let (mut result, snapshot) = self.query_state(query, thread)?;
        result["scope"] = if let Some(id) = thread {
            json!({"kind":"thread","threadId":id,"title":snapshot.as_ref().and_then(|s|s.threads.get(id)).map(|t|t.title.as_str()).unwrap_or("对话用量")})
        } else {
            json!({"kind":"local"})
        };
        let events = snapshot
            .as_ref()
            .map(|s| {
                s.events
                    .iter()
                    .filter(|e| {
                        e.thread_id.is_some()
                            && thread.is_none_or(|id| e.thread_id.as_deref() == Some(id))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let unavailable = snapshot.is_some()
            && events.is_empty()
            && result["coverage"]["unavailableThreads"]
                .as_u64()
                .unwrap_or(0)
                > 0;
        if snapshot.is_none() || unavailable {
            result["dataState"] = json!(if unavailable {
                "unavailable"
            } else {
                "indexing"
            });
            for key in ["lifetime", "today", "selected"] {
                result[key] = Value::Null;
            }
            result["daily"] = json!([]);
            result["models"] = json!([]);
            return Ok(result);
        }
        result["dataState"] = json!(if events.is_empty() {
            "empty"
        } else {
            "available"
        });
        result["lifetime"] = aggregate(events.iter().copied());
        result["today"] = aggregate(
            events
                .iter()
                .copied()
                .filter(|e| query.day(e) == query.today),
        );
        let selected = events
            .iter()
            .copied()
            .filter(|e| query.selected(e))
            .collect::<Vec<_>>();
        result["selected"] = aggregate(selected.iter().copied());
        let mut days = BTreeMap::<String, Vec<&UsageEvent>>::new();
        let mut models = BTreeMap::<Option<String>, Vec<&UsageEvent>>::new();
        for event in selected {
            days.entry(query.day(event)).or_default().push(event);
            models.entry(event.model.clone()).or_default().push(event);
        }
        result["daily"] = json!(days
            .into_iter()
            .map(|(date, entries)| json!({"date":date,"metrics":aggregate(entries.into_iter())}))
            .collect::<Vec<_>>());
        let mut models = models
            .into_iter()
            .map(|(model, entries)| json!({"model":model,"metrics":aggregate(entries.into_iter())}))
            .collect::<Vec<_>>();
        models.sort_by(|a, b| {
            b["metrics"]["totalTokens"]
                .as_i64()
                .cmp(&a["metrics"]["totalTokens"].as_i64())
                .then_with(|| a["model"].to_string().cmp(&b["model"].to_string()))
        });
        result["models"] = json!(models);
        Ok(result)
    }
    pub fn threads(
        self: &Arc<Self>,
        query: &UsageQuery,
        limit: usize,
        search: &str,
        cursor: Option<&str>,
    ) -> Result<Value> {
        if !(1..=100).contains(&limit) || search.encode_utf16().count() > 200 {
            return Err(BridgeError::invalid("每页条数或搜索内容无效。"));
        }
        let key = json!([query.period, query.time_zone, query.today, search]).to_string();
        let after = cursor.map(decode_cursor).transpose()?;
        if after.as_ref().is_some_and(|c| {
            c["key"] != key
                || !c["threadId"].is_string()
                || c["tokens"].as_i64().is_none_or(|n| n < -1)
        }) {
            return Err(BridgeError::invalid("分页位置与筛选不一致，请重新加载。"));
        }
        let (mut result, snapshot) = self.query_state(query, None)?;
        let Some(snapshot) = snapshot else {
            result["items"] = json!([]);
            result["nextCursor"] = Value::Null;
            result["coverage"]["unavailableThreads"] = json!(0);
            return Ok(result);
        };
        let mut groups = HashMap::<String, Vec<&UsageEvent>>::new();
        for event in &snapshot.events {
            if let Some(id) = &event.thread_id {
                groups.entry(id.clone()).or_default().push(event);
            }
        }
        let (present, gaps) = source_sets(&snapshot);
        let mut unavailable = 0;
        let mut items = vec![];
        for (id, thread) in &snapshot.threads {
            if !thread.title.to_lowercase().contains(&search.to_lowercase()) {
                continue;
            }
            let events = groups.get(id).cloned().unwrap_or_default();
            let missing = events.is_empty()
                && (thread.tokens > 0 || !present.contains(id) || gaps.contains(id));
            if missing {
                unavailable += 1;
            }
            items.push(json!({"threadId":id,"title":thread.title,"parentThreadId":thread.parent,"metrics":if missing{Value::Null}else{aggregate(events.iter().copied().filter(|e|query.selected(e)))},"status":if missing{"unavailable"}else if events.is_empty(){"empty"}else{"available"},"lastRecordedAt":events.iter().map(|e|&e.timestamp).max()}));
        }
        let tokens = |item: &Value| item["metrics"]["totalTokens"].as_i64().unwrap_or(-1);
        items.sort_by(|a, b| {
            tokens(b)
                .cmp(&tokens(a))
                .then_with(|| a["threadId"].as_str().cmp(&b["threadId"].as_str()))
        });
        if let Some(after) = after {
            let count = after["tokens"].as_i64().unwrap();
            let id = after["threadId"].as_str().unwrap();
            items.retain(|i| {
                tokens(i) < count || (tokens(i) == count && i["threadId"].as_str().unwrap() > id)
            });
        }
        let more = items.len() > limit;
        items.truncate(limit);
        result["nextCursor"] = json!(items.last().filter(|_| more).map(|last| encode_cursor(
            &json!({"key":key,"threadId":last["threadId"],"tokens":tokens(last)})
        )));
        result["items"] = json!(items);
        result["coverage"]["unavailableThreads"] = json!(unavailable);
        Ok(result)
    }
}
fn checkpoint(file: &mut fs::File, offset: u64, prefix_length: usize) -> Result<(String, String)> {
    let mut prefix = vec![0; prefix_length];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut prefix)?;
    let mut boundary = vec![0; offset.min(128) as usize];
    file.seek(SeekFrom::Start(offset - boundary.len() as u64))?;
    file.read_exact(&mut boundary)?;
    Ok((crate::digest(prefix), crate::digest(boundary)))
}
fn reconcile(db: &Connection, fingerprint: &str) -> Result<()> {
    let mut statement=db.prepare("SELECT owner,quality,event FROM usage_contributions WHERE fingerprint=? ORDER BY quality DESC")?;
    let rows = statement
        .query_map([fingerprint], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, u8>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if rows.is_empty() {
        db.execute(
            "DELETE FROM usage_events WHERE fingerprint=?",
            [fingerprint],
        )?;
        return Ok(());
    }
    let quality = rows
        .iter()
        .find(|(owner, _, _)| owner.is_some())
        .map(|(_, quality, _)| *quality);
    let best = rows
        .iter()
        .filter(|(owner, q, _)| owner.is_some() && Some(*q) == quality)
        .collect::<Vec<_>>();
    let owners = best
        .iter()
        .filter_map(|(owner, _, _)| owner.as_deref())
        .collect::<HashSet<_>>();
    let event: UsageEvent = serde_json::from_str(&best.first().unwrap_or(&&rows[0]).2)?;
    let mut models = HashSet::new();
    for (_, _, text) in &best {
        let event: UsageEvent = serde_json::from_str(text)?;
        if let Some(model) = event.model {
            models.insert(model);
        }
    }
    let model = if models.len() > 1 {
        None
    } else {
        models.into_iter().next().or(event.model)
    };
    let owner = if owners.len() == 1 {
        owners.into_iter().next()
    } else {
        None
    };
    db.execute(
        "INSERT OR REPLACE INTO usage_events VALUES (?,?,?,?,?)",
        params![
            fingerprint,
            owner,
            event.timestamp,
            model,
            event.metrics.to_string()
        ],
    )?;
    Ok(())
}
fn source_sets(snapshot: &Snapshot) -> (HashSet<String>, HashSet<String>) {
    let mut present = HashSet::new();
    let mut gaps = HashSet::new();
    for source in &snapshot.sources {
        if let Some(id) = &source.thread_id {
            if source.status == "ready" {
                present.insert(id.clone());
            }
            let s = &source.state;
            if s.gap_events + s.unassigned_events + s.unsupported_records + s.malformed_lines > 0 {
                gaps.insert(id.clone());
            }
        }
    }
    (present, gaps)
}
fn coverage(
    snapshot: Option<&Snapshot>,
    thread: Option<&str>,
    discovered: usize,
    processed: usize,
) -> Value {
    let Some(snapshot) = snapshot else {
        return json!({"discoveredSources":discovered,"processedSources":processed,"missingSources":0,"unreadableSources":0,"unsupportedSources":0,"unassignedEvents":0,"gapEvents":0,"unavailableThreads":0,"indexUnavailable":false,"firstRecordedAt":null,"lastRecordedAt":null});
    };
    let sources = snapshot
        .sources
        .iter()
        .filter(|s| thread.is_none_or(|id| s.thread_id.as_deref() == Some(id)))
        .collect::<Vec<_>>();
    let (present, gaps) = source_sets(snapshot);
    let events = snapshot
        .events
        .iter()
        .filter(|e| {
            e.thread_id.is_some() && thread.is_none_or(|id| e.thread_id.as_deref() == Some(id))
        })
        .collect::<Vec<_>>();
    let recorded = events
        .iter()
        .filter_map(|e| e.thread_id.as_ref())
        .collect::<HashSet<_>>();
    let unavailable = snapshot
        .threads
        .iter()
        .filter(|(id, t)| {
            thread.is_none_or(|i| *id == i)
                && !recorded.contains(id)
                && (t.tokens > 0 || !present.contains(*id) || gaps.contains(*id))
        })
        .count();
    json!({"discoveredSources":sources.len(),"processedSources":sources.len(),"missingSources":sources.iter().filter(|s|s.status=="missing"&&s.thread_id.as_ref().is_none_or(|id|!present.contains(id))).count(),"unreadableSources":sources.iter().filter(|s|s.status=="unreadable").count() as u64+snapshot.discovery_errors,
        "unsupportedSources":sources.iter().filter(|s|s.status=="unsupported").count(),"unassignedEvents":sources.iter().map(|s|s.state.unassigned_events).sum::<u64>()+if thread.is_some(){0}else{snapshot.events.iter().filter(|e|e.thread_id.is_none()).count() as u64},
        "gapEvents":sources.iter().map(|s|s.state.gap_events+s.state.malformed_lines+s.state.unsupported_records).sum::<u64>(),"unavailableThreads":unavailable,"indexUnavailable":snapshot.index_unavailable,"firstRecordedAt":events.iter().map(|e|&e.timestamp).min(),"lastRecordedAt":events.iter().map(|e|&e.timestamp).max()})
}
