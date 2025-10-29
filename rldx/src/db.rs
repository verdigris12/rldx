use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::BaseDirs;
use rusqlite::{params, Connection, TransactionBehavior};
use serde_json::Value;

use crate::search;

#[derive(Debug, Clone)]
pub struct IndexedItem {
    pub uuid: String,
    pub path: PathBuf,
    pub display_fn: String,
    pub rev: Option<String>,
    pub has_photo: bool,
    pub has_logo: bool,
    pub sha1: Vec<u8>,
    pub mtime: i64,
    pub lang_pref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IndexedProp {
    pub field: String,
    pub value: String,
    pub params: Value,
    pub seq: i64,
}

#[derive(Debug, Clone)]
pub struct StoredItem {
    pub uuid: String,
    pub path: PathBuf,
    pub sha1: Vec<u8>,
    pub mtime: i64,
}

#[derive(Debug, Clone)]
pub struct ContactListEntry {
    pub uuid: String,
    pub display_fn: String,
    pub path: PathBuf,
    pub has_photo: bool,
    pub has_logo: bool,
    pub primary_email: Option<String>,
    pub primary_org: Option<String>,
    pub kind: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ContactItem {
    pub uuid: String,
    pub path: PathBuf,
    pub display_fn: String,
    pub rev: Option<String>,
    pub has_photo: bool,
    pub has_logo: bool,
    pub lang_pref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PropRow {
    pub field: String,
    pub value: String,
    pub params: Value,
    pub seq: i64,
}

pub struct Database {
    conn: Connection,
    path: PathBuf,
}

impl Database {
    pub fn open() -> Result<Self> {
        let base = BaseDirs::new().context("unable to determine data directories")?;
        let data_dir = base.data_dir().join("rldx");
        fs::create_dir_all(&data_dir)?;
        let db_path = data_dir.join("index.db");
        let conn = Connection::open(&db_path)?;

        let mut db = Self {
            conn,
            path: db_path,
        };
        db.setup()?;
        Ok(db)
    }

    fn setup(&mut self) -> Result<()> {
        self.conn.pragma_update(None, "journal_mode", "WAL")?;
        self.conn.pragma_update(None, "synchronous", "FULL")?;
        self.conn.pragma_update(None, "foreign_keys", "ON")?;

        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS items (
              uuid TEXT PRIMARY KEY,
              path TEXT UNIQUE NOT NULL,
              fn   TEXT NOT NULL,
              rev  TEXT,
              has_photo INTEGER NOT NULL DEFAULT 0,
              has_logo  INTEGER NOT NULL DEFAULT 0,
              sha1 BLOB NOT NULL,
              mtime INTEGER NOT NULL,
              lang_pref TEXT
            );

            CREATE TABLE IF NOT EXISTS props (
              uuid  TEXT NOT NULL REFERENCES items(uuid) ON DELETE CASCADE,
              fn    TEXT NOT NULL,
              field TEXT NOT NULL,
              value TEXT NOT NULL,
              params TEXT DEFAULT '{}',
              seq   INTEGER NOT NULL DEFAULT 0,
              PRIMARY KEY (uuid, field, seq, value)
            );

            CREATE INDEX IF NOT EXISTS idx_items_fn ON items(fn);
            CREATE INDEX IF NOT EXISTS idx_props_field ON props(field);
            CREATE INDEX IF NOT EXISTS idx_props_value ON props(value);
            CREATE INDEX IF NOT EXISTS idx_props_fn ON props(fn);
        "#,
        )?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn upsert(&mut self, item: &IndexedItem, props: &[IndexedProp]) -> Result<()> {
        let mut tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        tx.execute(
            r#"
            INSERT INTO items (uuid, path, fn, rev, has_photo, has_logo, sha1, mtime, lang_pref)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(uuid) DO UPDATE SET
              path=excluded.path,
              fn=excluded.fn,
              rev=excluded.rev,
              has_photo=excluded.has_photo,
              has_logo=excluded.has_logo,
              sha1=excluded.sha1,
              mtime=excluded.mtime,
              lang_pref=excluded.lang_pref
        "#,
            params![
                item.uuid,
                item.path.to_string_lossy(),
                item.display_fn,
                item.rev,
                if item.has_photo { 1 } else { 0 },
                if item.has_logo { 1 } else { 0 },
                item.sha1,
                item.mtime,
                item.lang_pref,
            ],
        )?;

        tx.execute("DELETE FROM props WHERE uuid = ?1", params![item.uuid])?;

        {
            let mut stmt = tx.prepare(
                r#"INSERT INTO props (uuid, fn, field, value, params, seq)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
            )?;

            for prop in props {
                let params_json = serde_json::to_string(&prop.params).unwrap_or_else(|_| "{}".to_string());
                stmt.execute(params![
                    item.uuid,
                    item.display_fn,
                    prop.field,
                    prop.value,
                    params_json,
                    prop.seq,
                ])?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    pub fn stored_items(&self) -> Result<HashMap<PathBuf, StoredItem>> {
        let mut stmt = self.conn.prepare("SELECT uuid, path, sha1, mtime FROM items")?;
        let rows = stmt.query_map([], |row| {
            let path: String = row.get(1)?;
            Ok(StoredItem {
                uuid: row.get(0)?,
                path: PathBuf::from(path),
                sha1: row.get(2)?,
                mtime: row.get(3)?,
            })
        })?;

        let mut map = HashMap::new();
        for row in rows {
            let item = row?;
            map.insert(item.path.clone(), item);
        }
        Ok(map)
    }

    pub fn remove_missing(&self, existing_paths: &HashSet<PathBuf>) -> Result<()> {
        let mut stmt = self.conn.prepare("SELECT path FROM items")?;
        let rows = stmt.query_map([], |row| {
            let path: String = row.get(0)?;
            Ok(PathBuf::from(path))
        })?;

        let mut to_delete = Vec::new();
        for row in rows {
            let path = row?;
            if !existing_paths.contains(&path) {
                to_delete.push(path);
            }
        }

        for path in to_delete {
            self.conn
                .execute("DELETE FROM items WHERE path = ?1", params![path.to_string_lossy()])?;
        }
        Ok(())
    }

    pub fn list_contacts(&self, filter: Option<&str>) -> Result<Vec<ContactListEntry>> {
        let mut sql = String::from(
            "SELECT uuid, fn, path, has_photo, has_logo,
                    (SELECT value FROM props p WHERE p.uuid = items.uuid AND p.field = 'EMAIL' ORDER BY seq LIMIT 1),
                    (SELECT value FROM props p WHERE p.uuid = items.uuid AND p.field = 'ORG' ORDER BY seq LIMIT 1),
                    (SELECT value FROM props p WHERE p.uuid = items.uuid AND p.field = 'KIND' ORDER BY seq LIMIT 1)
             FROM items",
        );

        let mut args: Vec<String> = Vec::new();
        if let Some(filter) = filter {
            let pattern = search::like_pattern(filter);
            sql.push_str(
                " WHERE LOWER(fn) LIKE ?1 OR EXISTS (
                    SELECT 1 FROM props WHERE props.uuid = items.uuid
                      AND props.field IN ('NICKNAME','ORG','EMAIL','TEL')
                      AND LOWER(props.value) LIKE ?1
                 )",
            );
            args.push(pattern);
        }

        sql.push_str(" ORDER BY fn COLLATE NOCASE");

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = if args.is_empty() {
            stmt.query_map([], row_to_list_entry)?
        } else {
            stmt.query_map([args[0].as_str()], row_to_list_entry)?
        };

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get_contact(&self, uuid: &str) -> Result<Option<ContactItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT uuid, path, fn, rev, has_photo, has_logo, lang_pref FROM items WHERE uuid = ?1",
        )?;
        let mut rows = stmt.query([uuid])?;
        if let Some(row) = rows.next()? {
            let path: String = row.get(1)?;
            return Ok(Some(ContactItem {
                uuid: row.get(0)?,
                path: PathBuf::from(path),
                display_fn: row.get(2)?,
                rev: row.get(3)?,
                has_photo: row.get::<_, i64>(4)? != 0,
                has_logo: row.get::<_, i64>(5)? != 0,
                lang_pref: row.get(6)?,
            }));
        }
        Ok(None)
    }

    pub fn get_props(&self, uuid: &str) -> Result<Vec<PropRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT field, value, params, seq FROM props WHERE uuid = ?1 ORDER BY field, seq",
        )?;
        let rows = stmt.query_map([uuid], |row| {
            let raw: String = row.get(2)?;
            let params_json: Value = serde_json::from_str(&raw).unwrap_or_else(|_| Value::Object(Default::default()));
            Ok(PropRow {
                field: row.get(0)?,
                value: row.get(1)?,
                params: params_json,
                seq: row.get(3)?,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

fn row_to_list_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContactListEntry> {
    let path: String = row.get(2)?;
    Ok(ContactListEntry {
        uuid: row.get(0)?,
        display_fn: row.get(1)?,
        path: PathBuf::from(path),
        has_photo: row.get::<_, i64>(3)? != 0,
        has_logo: row.get::<_, i64>(4)? != 0,
        primary_email: row.get(5)?,
        primary_org: row.get(6)?,
        kind: row.get(7)?,
    })
}