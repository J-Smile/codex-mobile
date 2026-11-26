use crate::{BridgeError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{path::Path, sync::Mutex};

pub struct Operations {
    connection: Mutex<Connection>,
    _lock: std::fs::File,
}
pub enum Claim {
    Dispatch,
    Saved(Result<Value>),
}
impl Operations {
    pub fn open(directory: &Path) -> Result<Self> {
        std::fs::create_dir_all(directory)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("operations.lock"))?;
        lock.try_lock().map_err(|_| {
            BridgeError::unavailable("操作记录正在由另一手机服务使用，请先停止该服务。")
        })?;
        let connection = Connection::open(directory.join("operations.sqlite"))?;
        connection.busy_timeout(std::time::Duration::from_secs(2))?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS operations (
                owner TEXT NOT NULL, id TEXT NOT NULL, hash TEXT NOT NULL,
                instance TEXT NOT NULL, target TEXT NOT NULL, kind TEXT NOT NULL,
                phase TEXT NOT NULL, receipt TEXT, status INTEGER, closed INTEGER NOT NULL DEFAULT 0,
                updated INTEGER NOT NULL, PRIMARY KEY(owner,id));")?;
        // 没有完成回执的投递不能在进程重启后恢复执行。
        connection.execute(
            "UPDATE operations SET phase='unknown' WHERE phase='dispatching'",
            [],
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
            _lock: lock,
        })
    }
    pub fn claim(
        &self,
        owner: &str,
        id: &str,
        hash: &str,
        instance: &str,
        target: &str,
        kind: &str,
    ) -> Result<Claim> {
        let mut connection = self.connection.lock().unwrap();
        let transaction = connection.transaction()?;
        let existing = transaction
            .query_row(
                "SELECT hash,phase,receipt,status FROM operations WHERE owner=? AND id=?",
                params![owner, id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<u16>>(3)?,
                    ))
                },
            )
            .optional()?;
        let retry = if let Some((previous, phase, receipt, status)) = existing {
            if previous != hash {
                return Err(BridgeError::new(409, "操作编号已用于不同内容。"));
            }
            if phase == "accepted" {
                return Ok(Claim::Saved(Ok(serde_json::from_str(
                    receipt.as_deref().unwrap(),
                )?)));
            }
            if phase != "not-dispatched" {
                let mut error = BridgeError::new(
                    status.unwrap_or(409),
                    "此前操作结果尚未确认，请先在桌面检查；不会重复执行。",
                );
                error.outcome = Some("unknown".into());
                return Ok(Claim::Saved(Err(error)));
            }
            true
        } else {
            false
        };
        let unresolved: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM operations WHERE owner=? AND target=? AND phase IN ('dispatching','unknown') AND closed=0)", params![owner,target], |row| row.get(0))?;
        if unresolved {
            return Err(
                BridgeError::new(409, "该目标仍有未确认的操作，请先核对结果并结束等待。")
                    .before_send(),
            );
        }
        if retry {
            transaction.execute("UPDATE operations SET phase='dispatching',instance=?,updated=?,closed=0 WHERE owner=? AND id=?",params![instance,crate::now_ms(),owner,id])?;
        } else {
            transaction.execute("INSERT INTO operations(owner,id,hash,instance,target,kind,phase,updated) VALUES(?,?,?,?,?,?,'dispatching',?)",params![owner,id,hash,instance,target,kind,crate::now_ms()])?;
        }
        transaction.commit()?;
        Ok(Claim::Dispatch)
    }
    pub fn finish(&self, owner: &str, id: &str, result: &Result<Value>) -> Result<()> {
        // 只保存必要的回执，不落提示词、附件或完整轮次内容。
        let (phase, receipt, status) = match result {
            Ok(value) => ("accepted", Some(json!({"status":value["status"],"messageId":value["messageId"],"threadId":value["threadId"],"turnId":value["turn"]["id"],"firstTurnAccepted":value["firstTurnAccepted"],"firstTurnPending":value["firstTurnPending"],"execution":value["execution"],"resolved":value["resolved"],"accepted":value["status"]!="created"}).to_string()), None),
            Err(error) if error.before_send => ("not-dispatched", None, Some(error.status)),
            Err(error) => ("unknown", None, Some(error.status)),
        };
        self.connection.lock().unwrap().execute(
            "UPDATE operations SET phase=?,receipt=?,status=?,updated=? WHERE owner=? AND id=?",
            params![phase, receipt, status, crate::now_ms(), owner, id],
        )?;
        Ok(())
    }
    pub fn pending(&self, owner: &str) -> Result<Vec<Value>> {
        let connection = self.connection.lock().unwrap();
        let mut statement = connection.prepare("SELECT id,kind,target,phase,updated FROM operations WHERE owner=? AND phase IN ('dispatching','unknown') AND closed=0 ORDER BY updated")?;
        let rows = statement.query_map([owner], |row| Ok(json!({"id":row.get::<_,String>(0)?,"kind":row.get::<_,String>(1)?,"target":row.get::<_,String>(2)?,"phase":row.get::<_,String>(3)?,"updatedAt":row.get::<_,i64>(4)?})))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
    /// 仅按当前拥有者查询；没有记录不代表尚未投递。
    pub fn lookup(&self, owner: &str, ids: &[String]) -> Result<Vec<Value>> {
        let connection = self.connection.lock().unwrap();
        let mut statement = connection.prepare("SELECT kind,target,phase,receipt,closed,updated FROM operations WHERE owner=? AND id=?")?;
        ids.iter().map(|id| {
            let row = statement.query_row(params![owner, id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, Option<String>>(3)?, row.get::<_, bool>(4)?, row.get::<_, i64>(5)?))
            }).optional()?;
            match row {
                Some((kind, target, phase, receipt, closed, updated)) => {
                    let receipt: Option<Value> = receipt.map(|value| serde_json::from_str(&value)).transpose()?;
                    Ok(json!({"id":id,"kind":kind,"target":target,"phase":phase,"receipt":receipt,"closed":closed,"updatedAt":updated}))
                }
                None => Ok(json!({"id":id,"phase":"missing"})),
            }
        }).collect()
    }
    /// 创建回执与官方观察到的首轮分开保存，读到空聊天不触发再次创建。
    pub fn creation_pending(&self, thread: &str, observed_turn: Option<&str>) -> Result<bool> {
        let mut connection = self.connection.lock().unwrap();
        let transaction = connection.transaction()?;
        let rows = {
            let mut statement = transaction.prepare("SELECT owner,id,receipt FROM operations WHERE kind='create' AND phase='accepted' AND json_extract(receipt,'$.threadId')=? AND json_extract(receipt,'$.firstTurnAccepted')=0")?;
            let rows = statement.query_map([thread], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut pending = false;
        for (owner, id, receipt) in rows {
            let mut value: Value = serde_json::from_str(&receipt)?;
            if value["threadId"] != thread || value["firstTurnAccepted"] == true {
                continue;
            }
            if value["execution"] == "mobile" && value["firstTurnPending"] == false {
                continue;
            }
            if let Some(turn) = observed_turn {
                value["firstTurnAccepted"] = json!(true);
                value["firstTurnId"] = json!(turn);
                transaction.execute(
                    "UPDATE operations SET receipt=?,updated=? WHERE owner=? AND id=?",
                    params![value.to_string(), crate::now_ms(), owner, id],
                )?;
            } else {
                pending = true;
            }
        }
        transaction.commit()?;
        Ok(pending)
    }
    pub fn close_unknown(&self, owner: &str, id: &str) -> Result<()> {
        let changed = self.connection.lock().unwrap().execute("UPDATE operations SET closed=1 WHERE owner=? AND id=? AND phase='unknown' AND closed=0",params![owner,id])?;
        if changed != 1 {
            return Err(BridgeError::new(409, "该操作仍在处理中，或已结案。"));
        }
        Ok(())
    }
    pub fn interrupted(&self) -> Result<()> {
        self.connection.lock().unwrap().execute(
            "UPDATE operations SET phase='unknown' WHERE phase='dispatching'",
            [],
        )?;
        Ok(())
    }
    pub fn release(&self) -> Result<()> {
        self._lock.unlock()?;
        Ok(())
    }
}
