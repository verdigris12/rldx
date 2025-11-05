use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::BaseDirs;
use rusqlite::{params, Connection, Row, TransactionBehavior};
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
    pub path: PathBuf,
    pub sha1: Vec<u8>,
    pub mtime: i64,
}

#[derive(Debug, Clone)]
pub struct ContactListEntry {
    pub uuid: String,
    pub display_fn: String,
    pub path: PathBuf,
    pub primary_org: Option<String>,
    pub kind: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ContactItem {
    pub path: PathBuf,
    pub display_fn: String,
    pub has_photo: bool,
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
}

impl Database {
    pub fn open() -> Result<Self> {
        let base = BaseDirs::new().context("unable to determine data directories")?;
        let data_dir = base.data_dir().join("rldx");
        fs::create_dir_all(&data_dir)?;
        let db_path = data_dir.join("index.db");
        let conn = Connection::open(&db_path)?;

        let mut db = Self { conn };
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
              fn_norm TEXT,
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
              value_norm TEXT,
              params TEXT DEFAULT '{}',
              seq   INTEGER NOT NULL DEFAULT 0,
              PRIMARY KEY (uuid, field, seq, value)
            );

            CREATE INDEX IF NOT EXISTS idx_items_fn ON items(fn);
            CREATE INDEX IF NOT EXISTS idx_items_fn_norm ON items(fn_norm);
            CREATE INDEX IF NOT EXISTS idx_props_field ON props(field);
            CREATE INDEX IF NOT EXISTS idx_props_value ON props(value);
            CREATE INDEX IF NOT EXISTS idx_props_value_norm ON props(value_norm);
            CREATE INDEX IF NOT EXISTS idx_props_fn ON props(fn);
        "#,
        )?;

        // Migration for existing DBs: ensure columns exist and backfill
        self.ensure_norm_columns()?;
        self.backfill_norm_columns()?;
        Ok(())
    }

    fn column_exists(&self, table: &str, column: &str) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare(&format!("PRAGMA table_info({})", table))?;
        let rows = stmt.query_map([], |row: &Row| -> rusqlite::Result<String> { row.get(1) })?;
        for r in rows {
            if r? == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn ensure_norm_columns(&mut self) -> Result<()> {
        if !self.column_exists("items", "fn_norm")? {
            let _ = self
                .conn
                .execute_batch("ALTER TABLE items ADD COLUMN fn_norm TEXT;")?;
        }
        if !self.column_exists("props", "value_norm")? {
            let _ = self
                .conn
                .execute_batch("ALTER TABLE props ADD COLUMN value_norm TEXT;")?;
        }
        Ok(())
    }

    fn backfill_norm_columns(&mut self) -> Result<()> {
        // items.fn_norm backfill
        let items_to_update: Vec<(String, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT uuid, fn FROM items WHERE fn_norm IS NULL OR fn_norm = ''")?;
            let rows = stmt.query_map([], |row| {
                let uuid: String = row.get(0)?;
                let fun: String = row.get(1)?;
                Ok((uuid, fun))
            })?;
            let mut acc = Vec::new();
            for r in rows {
                acc.push(r?);
            }
            acc
        };
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut upd = tx.prepare("UPDATE items SET fn_norm = ?1 WHERE uuid = ?2")?;
            for (uuid, fun) in items_to_update {
                let norm = fun.to_lowercase();
                let _ = upd.execute(params![norm, uuid])?;
            }
        }
        tx.commit()?;

        // props.value_norm backfill
        let props_to_update: Vec<(String, String, i64, String)> = {
            let mut stmt2 = self.conn.prepare(
                "SELECT uuid, field, seq, value FROM props WHERE value_norm IS NULL OR value_norm = ''",
            )?;
            let rows2 = stmt2.query_map([], |row| {
                let uuid: String = row.get(0)?;
                let field: String = row.get(1)?;
                let seq: i64 = row.get(2)?;
                let value: String = row.get(3)?;
                Ok((uuid, field, seq, value))
            })?;
            let mut acc = Vec::new();
            for r in rows2 {
                acc.push(r?);
            }
            acc
        };
        let tx2 = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut upd = tx2.prepare(
                "UPDATE props SET value_norm = ?1 WHERE uuid = ?2 AND field = ?3 AND seq = ?4 AND value = ?5",
            )?;
            for (uuid, field, seq, value) in props_to_update {
                let norm = value.to_lowercase();
                let _ = upd.execute(params![norm, uuid, field, seq, value])?;
            }
        }
        tx2.commit()?;

        Ok(())
    }

    pub fn upsert(&mut self, item: &IndexedItem, props: &[IndexedProp]) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        tx.execute(
            r#"
            INSERT INTO items (uuid, path, fn, fn_norm, rev, has_photo, has_logo, sha1, mtime, lang_pref)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(uuid) DO UPDATE SET
              path=excluded.path,
              fn=excluded.fn,
              fn_norm=excluded.fn_norm,
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
                item.display_fn.to_lowercase(),
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
                r#"INSERT INTO props (uuid, fn, field, value, value_norm, params, seq)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
            )?;

            for prop in props {
                let params_json =
                    serde_json::to_string(&prop.params).unwrap_or_else(|_| "{}".to_string());
                stmt.execute(params![
                    item.uuid,
                    item.display_fn,
                    prop.field,
                    prop.value,
                    prop.value.to_lowercase(),
                    params_json,
                    prop.seq,
                ])?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    pub fn stored_items(&self) -> Result<HashMap<PathBuf, StoredItem>> {
        let mut stmt = self.conn.prepare("SELECT path, sha1, mtime FROM items")?;
        let rows = stmt.query_map([], |row| {
            let path: String = row.get(0)?;
            Ok(StoredItem {
                path: PathBuf::from(path),
                sha1: row.get(1)?,
                mtime: row.get(2)?,
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
            self.conn.execute(
                "DELETE FROM items WHERE path = ?1",
                params![path.to_string_lossy()],
            )?;
        }
        Ok(())
    }

    pub fn delete_items_by_paths<I>(&mut self, paths: I) -> Result<()>
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut stmt = tx.prepare("DELETE FROM items WHERE path = ?1")?;
            for path in paths {
                let _ = stmt.execute(params![path.to_string_lossy()])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_contacts(&self, filter: Option<&str>) -> Result<Vec<ContactListEntry>> {
        let mut sql = String::from(
            "SELECT uuid, fn, path,
                    (SELECT value FROM props p WHERE p.uuid = items.uuid AND p.field = 'ORG' ORDER BY seq LIMIT 1),
                    (SELECT value FROM props p WHERE p.uuid = items.uuid AND p.field = 'KIND' ORDER BY seq LIMIT 1)
             FROM items",
        );

        let mut args: Vec<String> = Vec::new();
        if let Some(filter) = filter {
            let pattern = search::like_pattern(filter);
            sql.push_str(
                " WHERE fn_norm LIKE ?1 OR EXISTS (
                    SELECT 1 FROM props WHERE props.uuid = items.uuid
                      AND props.field IN ('NICKNAME','ORG','EMAIL','TEL')
                      AND props.value_norm LIKE ?1
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
        let mut stmt = self
            .conn
            .prepare("SELECT path, fn, has_photo, lang_pref FROM items WHERE uuid = ?1")?;
        let mut rows = stmt.query([uuid])?;
        if let Some(row) = rows.next()? {
            let path: String = row.get(0)?;
            return Ok(Some(ContactItem {
                path: PathBuf::from(path),
                display_fn: row.get(1)?,
                has_photo: row.get::<_, i64>(2)? != 0,
                lang_pref: row.get(3)?,
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
            let params_json: Value =
                serde_json::from_str(&raw).unwrap_or_else(|_| Value::Object(Default::default()));
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
        primary_org: row.get(3)?,
        kind: row.get(4)?,
    })
}
