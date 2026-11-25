use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{params, types::Type, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Mutex, time::Duration};
use uuid::Uuid;
use webauthn_rs::prelude::{AuthenticationResult, CredentialID, Passkey};

pub struct Accounts(Mutex<Connection>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountKeys {
    pub name: String,
    pub login_key: String,
    pub desktop_key: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct Account {
    pub id: String,
    pub name: String,
}

#[derive(Serialize)]
pub(crate) struct ListedDesktop {
    pub id: String,
    pub name: String,
    pub online: bool,
}

pub(crate) type SessionKey = [u8; 32];
pub(crate) const SESSION_SECONDS: i64 = 30 * 24 * 60 * 60;

fn issue_session(
    db: &Connection,
    account_id: &str,
    key: SessionKey,
    previous: Option<SessionKey>,
) -> rusqlite::Result<()> {
    db.execute(
        "DELETE FROM sessions WHERE expires <= unixepoch() OR key = ?1",
        [previous],
    )?;
    db.execute(
        "INSERT INTO sessions(key, account_id, expires) VALUES (?1, ?2, unixepoch() + ?3)",
        params![key, account_id, SESSION_SECONDS],
    )?;
    Ok(())
}

fn encode_passkey(passkey: &Passkey) -> rusqlite::Result<String> {
    serde_json::to_string(passkey)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

pub(crate) fn digest(key: &str) -> SessionKey {
    Sha256::digest(key.as_bytes()).into()
}

pub(crate) fn random_key() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

impl Accounts {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(3))?;
        db.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS accounts (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL COLLATE NOCASE UNIQUE,
                 login_key BLOB NOT NULL UNIQUE,
                 desktop_key BLOB NOT NULL UNIQUE
             );
             CREATE TABLE IF NOT EXISTS sessions (
                 key BLOB PRIMARY KEY,
                 account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 expires INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS desktops (
                 account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 id TEXT NOT NULL,
                 name TEXT NOT NULL,
                 PRIMARY KEY (account_id, id)
             );
             CREATE TABLE IF NOT EXISTS passkeys (
                 credential_id BLOB PRIMARY KEY NOT NULL,
                 account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 credential TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS passkeys_by_account ON passkeys(account_id);
             CREATE TABLE IF NOT EXISTS desktop_approvers (
                 account_id TEXT NOT NULL,
                 desktop_id TEXT NOT NULL,
                 key BLOB NOT NULL,
                 PRIMARY KEY (account_id, desktop_id),
                 FOREIGN KEY (account_id, desktop_id) REFERENCES desktops(account_id, id) ON DELETE CASCADE
             );
             CREATE TABLE IF NOT EXISTS desktop_credentials (
                 key BLOB PRIMARY KEY NOT NULL,
                 account_id TEXT NOT NULL,
                 desktop_id TEXT NOT NULL,
                 UNIQUE (account_id, desktop_id),
                 FOREIGN KEY (account_id, desktop_id) REFERENCES desktops(account_id, id) ON DELETE CASCADE
             );",
        )?;
        Ok(Self(Mutex::new(db)))
    }

    pub fn create(&self, name: &str) -> Result<AccountKeys, Box<dyn std::error::Error>> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 48 || name.chars().any(char::is_control) {
            return Err("账号名称需是 1 至 48 个字符。".into());
        }
        let id = Uuid::new_v4().simple().to_string();
        let login_key = format!("cm_login_{}", random_key());
        let desktop_key = format!("cm_desktop_{}", random_key());
        self.0.lock().unwrap().execute(
            "INSERT INTO accounts(id, name, login_key, desktop_key) VALUES (?1, ?2, ?3, ?4)",
            params![id, name, digest(&login_key), digest(&desktop_key)],
        )?;
        Ok(AccountKeys {
            name: name.into(),
            login_key,
            desktop_key,
        })
    }

    pub fn remove(&self, name: &str) -> rusqlite::Result<bool> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .execute("DELETE FROM accounts WHERE name = ?1", [name])?
            != 0)
    }

    pub(crate) fn login(
        &self,
        login_key: &str,
        key: SessionKey,
        previous: Option<SessionKey>,
    ) -> rusqlite::Result<Option<Account>> {
        let mut db = self.0.lock().unwrap();
        let tx = db.transaction()?;
        let account = tx
            .query_row(
                "SELECT id, name FROM accounts WHERE login_key = ?1",
                [digest(login_key)],
                |row| {
                    Ok(Account {
                        id: row.get(0)?,
                        name: row.get(1)?,
                    })
                },
            )
            .optional()?;
        let Some(account) = account else {
            return Ok(None);
        };
        issue_session(&tx, &account.id, key, previous)?;
        tx.commit()?;
        Ok(Some(account))
    }

    pub(crate) fn passkey_ids(&self, account: &str) -> rusqlite::Result<Vec<CredentialID>> {
        let db = self.0.lock().unwrap();
        let mut query = db.prepare("SELECT credential_id FROM passkeys WHERE account_id = ?1")?;
        let rows = query.query_map([account], |row| row.get(0))?;
        rows.collect()
    }

    pub(crate) fn has_passkey(&self, account: &str) -> rusqlite::Result<bool> {
        self.0.lock().unwrap().query_row(
            "SELECT EXISTS(SELECT 1 FROM passkeys WHERE account_id = ?1)",
            [account],
            |row| row.get(0),
        )
    }

    pub(crate) fn sign_up_with_passkey(
        &self,
        account: &Account,
        passkey: &Passkey,
        key: SessionKey,
    ) -> rusqlite::Result<()> {
        let mut db = self.0.lock().unwrap();
        let tx = db.transaction()?;
        // 新账号用通行密钥登录。两列保留随机占位值以兼容现存数据库。
        tx.execute(
            "INSERT INTO accounts(id, name, login_key, desktop_key) VALUES (?1, ?2, ?3, ?4)",
            params![
                account.id,
                account.name,
                digest(&random_key()),
                digest(&random_key())
            ],
        )?;
        tx.execute(
            "INSERT INTO passkeys(credential_id, account_id, credential) VALUES (?1, ?2, ?3)",
            params![passkey.cred_id(), account.id, encode_passkey(passkey)?],
        )?;
        issue_session(&tx, &account.id, key, None)?;
        tx.commit()
    }

    pub(crate) fn login_with_desktop(
        &self,
        account_id: &str,
        desktop_id: &str,
        key: SessionKey,
        previous: Option<SessionKey>,
    ) -> rusqlite::Result<Option<Account>> {
        let mut db = self.0.lock().unwrap();
        let tx = db.transaction()?;
        let name = tx
            .query_row(
                "SELECT accounts.name FROM accounts
             JOIN desktops ON desktops.account_id = accounts.id
             WHERE accounts.id = ?1 AND desktops.id = ?2",
                params![account_id, desktop_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(name) = name else {
            return Ok(None);
        };
        issue_session(&tx, account_id, key, previous)?;
        tx.commit()?;
        Ok(Some(Account {
            id: account_id.into(),
            name,
        }))
    }

    pub(crate) fn enable_desktop_approval(
        &self,
        account: &str,
        desktop: &str,
        login_key: &str,
        approver_key: SessionKey,
    ) -> rusqlite::Result<bool> {
        Ok(self.0.lock().unwrap().execute(
            "INSERT INTO desktop_approvers(account_id, desktop_id, key)
             SELECT accounts.id, desktops.id, ?4 FROM accounts
             JOIN desktops ON desktops.account_id = accounts.id
             WHERE accounts.id = ?1 AND desktops.id = ?2 AND accounts.login_key = ?3
             ON CONFLICT(account_id, desktop_id) DO UPDATE SET key = excluded.key",
            params![account, desktop, digest(login_key), approver_key],
        )? == 1)
    }

    pub(crate) fn desktop_approver(
        &self,
        account: &str,
        desktop: &str,
        key: SessionKey,
    ) -> rusqlite::Result<bool> {
        self.0.lock().unwrap().query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM desktop_approvers WHERE account_id = ?1 AND desktop_id = ?2 AND key = ?3
             )",
            params![account, desktop, key],
            |row| row.get(0),
        )
    }

    pub(crate) fn add_passkey(
        &self,
        session: SessionKey,
        passkey: &Passkey,
    ) -> rusqlite::Result<bool> {
        let credential = encode_passkey(passkey)?;
        let count = self.0.lock().unwrap().execute(
            "INSERT INTO passkeys(credential_id, account_id, credential)
             SELECT ?1, account_id, ?2 FROM sessions WHERE key = ?3 AND expires > unixepoch()
             ON CONFLICT(credential_id) DO NOTHING",
            params![passkey.cred_id(), credential, session],
        )?;
        Ok(count == 1)
    }

    pub(crate) fn login_with_passkey(
        &self,
        account_id: &str,
        credential_id: &[u8],
        verify: impl FnOnce(&Passkey) -> Option<AuthenticationResult>,
        key: SessionKey,
        previous: Option<SessionKey>,
    ) -> rusqlite::Result<Option<Account>> {
        let mut db = self.0.lock().unwrap();
        let tx = db.transaction()?;
        let saved = tx
            .query_row(
                "SELECT accounts.name, passkeys.credential FROM passkeys
             JOIN accounts ON accounts.id = passkeys.account_id
             WHERE passkeys.account_id = ?1 AND passkeys.credential_id = ?2",
                params![account_id, credential_id],
                |row| {
                    let name: String = row.get(0)?;
                    let encoded: String = row.get(1)?;
                    let passkey: Passkey = serde_json::from_str(&encoded).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(1, Type::Text, Box::new(error))
                    })?;
                    Ok((name, passkey))
                },
            )
            .optional()?;
        let Some((name, mut passkey)) = saved else {
            return Ok(None);
        };
        let Some(result) = verify(&passkey) else {
            return Ok(None);
        };
        if passkey.update_credential(&result) == Some(true) {
            tx.execute(
                "UPDATE passkeys SET credential = ?1 WHERE credential_id = ?2",
                params![encode_passkey(&passkey)?, credential_id],
            )?;
        }
        issue_session(&tx, account_id, key, previous)?;
        tx.commit()?;
        Ok(Some(Account {
            id: account_id.to_owned(),
            name,
        }))
    }

    pub(crate) fn session(&self, key: SessionKey) -> rusqlite::Result<Option<(Account, u64)>> {
        self.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT accounts.id, accounts.name, sessions.expires - unixepoch()
             FROM sessions JOIN accounts ON accounts.id = sessions.account_id
             WHERE sessions.key = ?1 AND sessions.expires > unixepoch()",
                [key],
                |row| {
                    Ok((
                        Account {
                            id: row.get(0)?,
                            name: row.get(1)?,
                        },
                        row.get(2)?,
                    ))
                },
            )
            .optional()
    }

    pub(crate) fn logout(&self, key: SessionKey) -> rusqlite::Result<()> {
        self.0
            .lock()
            .unwrap()
            .execute("DELETE FROM sessions WHERE key = ?1", [key])?;
        Ok(())
    }

    pub(crate) fn desktop_owner(
        &self,
        key: &str,
        desktop_id: &str,
    ) -> rusqlite::Result<Option<String>> {
        let sql = if key.starts_with("cm_device_") {
            "SELECT account_id FROM desktop_credentials WHERE key = ?1 AND desktop_id = ?2"
        } else {
            "SELECT id FROM accounts WHERE desktop_key = ?1 AND NOT EXISTS (
                 SELECT 1 FROM desktop_credentials WHERE account_id = accounts.id AND desktop_id = ?2
             )"
        };
        self.0
            .lock()
            .unwrap()
            .query_row(sql, params![digest(key), desktop_id], |row| row.get(0))
            .optional()
    }

    pub(crate) fn linked_desktop(
        &self,
        key: SessionKey,
        desktop_id: &str,
    ) -> rusqlite::Result<bool> {
        self.0.lock().unwrap().query_row(
            "SELECT EXISTS(SELECT 1 FROM desktop_credentials WHERE key = ?1 AND desktop_id = ?2)",
            params![key, desktop_id],
            |row| row.get(0),
        )
    }

    pub(crate) fn link_desktop(
        &self,
        session: SessionKey,
        id: &str,
        name: &str,
        desktop_key: SessionKey,
    ) -> rusqlite::Result<Option<bool>> {
        let mut db = self.0.lock().unwrap();
        let tx = db.transaction()?;
        let account: Option<String> = tx
            .query_row(
                "SELECT account_id FROM sessions WHERE key = ?1 AND expires > unixepoch()",
                [session],
                |row| row.get(0),
            )
            .optional()?;
        let Some(account) = account else {
            return Ok(None);
        };
        let inserted = tx.execute(
            "INSERT INTO desktops(account_id, id, name) VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id, id) DO NOTHING",
            params![account, id, name],
        )?;
        if inserted == 0 {
            return Ok(Some(false));
        }
        let inserted = tx.execute(
            "INSERT INTO desktop_credentials(key, account_id, desktop_id) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO NOTHING",
            params![desktop_key, account, id],
        )?;
        if inserted == 0 {
            return Ok(Some(false));
        }
        tx.commit()?;
        Ok(Some(true))
    }

    pub(crate) fn register_desktop(
        &self,
        account: &str,
        id: &str,
        name: &str,
    ) -> rusqlite::Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT INTO desktops(account_id, id, name) VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id, id) DO UPDATE SET name = excluded.name",
            params![account, id, name],
        )?;
        Ok(())
    }

    pub(crate) fn desktops(&self, account: &str) -> rusqlite::Result<Vec<ListedDesktop>> {
        let db = self.0.lock().unwrap();
        let mut query = db.prepare(
            "SELECT id, name FROM desktops WHERE account_id = ?1 ORDER BY name COLLATE NOCASE, id",
        )?;
        let rows = query.query_map([account], |row| {
            Ok(ListedDesktop {
                id: row.get(0)?,
                name: row.get(1)?,
                online: false,
            })
        })?;
        rows.collect()
    }
}
