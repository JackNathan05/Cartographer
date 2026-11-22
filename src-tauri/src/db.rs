//! SQLite index: one row per folder, one row per big file, one row per scan.

use crate::scan::Dir;
use rusqlite::{params, Connection, OptionalExtension, Statement};
use serde::Serialize;
use std::path::Path;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let c = Connection::open(path)?;
    c.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS scans(
           id INTEGER PRIMARY KEY, root TEXT NOT NULL, started INTEGER, duration_ms INTEGER,
           dirs INTEGER, files INTEGER, bytes INTEGER, denied INTEGER, root_dir INTEGER);
         CREATE TABLE IF NOT EXISTS dirs(
           id INTEGER PRIMARY KEY, scan_id INTEGER, parent_id INTEGER, name TEXT, path TEXT,
           size INTEGER, files INTEGER, dirs INTEGER, newest INTEGER,
           small_bytes INTEGER, small_count INTEGER, denied INTEGER);
         CREATE INDEX IF NOT EXISTS dirs_parent ON dirs(parent_id);
         CREATE INDEX IF NOT EXISTS dirs_scan ON dirs(scan_id);
         CREATE TABLE IF NOT EXISTS files(
           id INTEGER PRIMARY KEY, dir_id INTEGER, scan_id INTEGER, name TEXT, size INTEGER, modified INTEGER);
         CREATE INDEX IF NOT EXISTS files_dir ON files(dir_id);
         CREATE INDEX IF NOT EXISTS files_scan ON files(scan_id);",
    )?;
    // v0.1 databases: add the scan method column ("walk" or "fast")
    let _ = c.execute("ALTER TABLE scans ADD COLUMN method TEXT", []);
    Ok(c)
}

fn insert_tree(d: &mut Statement, f: &mut Statement, scan: i64, parent: Option<i64>, n: &Dir) -> rusqlite::Result<i64> {
    let id = d.insert(params![
        scan, parent, n.name, n.path, n.size as i64, n.files as i64, n.dirs as i64,
        n.newest, n.small_bytes as i64, n.small_count as i64, n.denied
    ])?;
    for file in &n.big {
        f.execute(params![id, scan, file.name, file.size as i64, file.modified])?;
    }
    for c in &n.children {
        insert_tree(d, f, scan, Some(id), c)?;
    }
    Ok(id)
}

/// Replaces any previous scan of the same root.
pub fn save_scan(conn: &mut Connection, root: &str, started: i64, duration_ms: i64, denied: u64, method: &str, tree: &Dir) -> rusqlite::Result<i64> {
    let tx = conn.transaction()?;
    let old: Vec<i64> = {
        let mut s = tx.prepare("SELECT id FROM scans WHERE root = ?1")?;
        let rows = s.query_map([root], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    for id in old {
        tx.execute("DELETE FROM files WHERE scan_id = ?1", [id])?;
        tx.execute("DELETE FROM dirs WHERE scan_id = ?1", [id])?;
        tx.execute("DELETE FROM scans WHERE id = ?1", [id])?;
    }
    tx.execute(
        "INSERT INTO scans(root, started, duration_ms, dirs, files, bytes, denied, method) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![root, started, duration_ms, tree.dirs as i64, tree.files as i64, tree.size as i64, denied as i64, method],
    )?;
    let scan_id = tx.last_insert_rowid();
    let root_dir = {
        let mut d = tx.prepare(
            "INSERT INTO dirs(scan_id, parent_id, name, path, size, files, dirs, newest, small_bytes, small_count, denied)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        )?;
        let mut f = tx.prepare("INSERT INTO files(dir_id, scan_id, name, size, modified) VALUES (?1,?2,?3,?4,?5)")?;
        insert_tree(&mut d, &mut f, scan_id, None, tree)?
    };
    tx.execute("UPDATE scans SET root_dir = ?1 WHERE id = ?2", params![root_dir, scan_id])?;
    tx.commit()?;
    Ok(scan_id)
}

#[derive(Serialize, Clone)]
pub struct ScanInfo {
    pub id: i64,
    pub root: String,
    pub started: i64,
    pub duration_ms: i64,
    pub dirs: i64,
    pub files: i64,
    pub bytes: i64,
    pub denied: i64,
    pub root_dir: i64,
    pub method: String,
}

pub fn scans(conn: &Connection) -> rusqlite::Result<Vec<ScanInfo>> {
    let mut s = conn.prepare(
        "SELECT id, root, started, duration_ms, dirs, files, bytes, denied, root_dir, COALESCE(method, 'walk') FROM scans ORDER BY root",
    )?;
    let rows = s.query_map([], |r| {
        Ok(ScanInfo {
            id: r.get(0)?, root: r.get(1)?, started: r.get(2)?, duration_ms: r.get(3)?,
            dirs: r.get(4)?, files: r.get(5)?, bytes: r.get(6)?, denied: r.get(7)?, root_dir: r.get(8)?, method: r.get(9)?,
        })
    })?;
    rows.collect()
}

#[derive(Serialize, Clone)]
pub struct Item {
    pub id: Option<i64>,
    pub kind: &'static str, // "dir" | "file" | "other" | "hidden"
    pub name: String,
    pub path: Option<String>,
    pub size: i64,
    pub files: i64,
    pub newest: i64,
    pub denied: bool,
    pub children: Option<Vec<Item>>,
}

#[derive(Serialize)]
pub struct Crumb {
    pub id: i64,
    pub name: String,
}

#[derive(Serialize)]
pub struct View {
    pub scan_id: i64,
    pub root: String,
    pub node: Item,
    pub crumbs: Vec<Crumb>,
}

struct DirRow {
    id: i64,
    parent: Option<i64>,
    name: String,
    path: String,
    size: i64,
    files: i64,
    newest: i64,
    small_bytes: i64,
    small_count: i64,
    denied: bool,
}

fn dir_row(conn: &Connection, id: i64) -> rusqlite::Result<Option<DirRow>> {
    conn.query_row(
        "SELECT id, parent_id, name, path, size, files, newest, small_bytes, small_count, denied FROM dirs WHERE id = ?1",
        [id],
        |r| {
            Ok(DirRow {
                id: r.get(0)?, parent: r.get(1)?, name: r.get(2)?, path: r.get(3)?, size: r.get(4)?,
                files: r.get(5)?, newest: r.get(6)?, small_bytes: r.get(7)?, small_count: r.get(8)?, denied: r.get(9)?,
            })
        },
    )
    .optional()
}

/// A folder with its largest children (and their children, `depth` levels down).
/// Whatever doesn't make the cut is summed into one "smaller items" block so sizes always add up.
fn load(conn: &Connection, row: DirRow, depth: u32, limit: usize) -> rusqlite::Result<Item> {
    let mut kids: Vec<Item> = Vec::new();
    {
        let mut s = conn.prepare_cached(
            "SELECT id, parent_id, name, path, size, files, newest, small_bytes, small_count, denied
             FROM dirs WHERE parent_id = ?1 ORDER BY size DESC LIMIT ?2",
        )?;
        let rows = s.query_map(params![row.id, limit as i64], |r| {
            Ok(DirRow {
                id: r.get(0)?, parent: r.get(1)?, name: r.get(2)?, path: r.get(3)?, size: r.get(4)?,
                files: r.get(5)?, newest: r.get(6)?, small_bytes: r.get(7)?, small_count: r.get(8)?, denied: r.get(9)?,
            })
        })?;
        let dirs: Vec<DirRow> = rows.collect::<Result<_, _>>()?;
        for (i, c) in dirs.into_iter().enumerate() {
            if depth > 0 && i < 40 && c.size > 0 {
                kids.push(load(conn, c, depth - 1, 30)?);
            } else {
                kids.push(Item {
                    id: Some(c.id), kind: "dir", name: c.name, path: Some(c.path), size: c.size,
                    files: c.files, newest: c.newest, denied: c.denied, children: None,
                });
            }
        }
    }
    {
        let mut s = conn.prepare_cached("SELECT name, size, modified FROM files WHERE dir_id = ?1 ORDER BY size DESC LIMIT ?2")?;
        let rows = s.query_map(params![row.id, limit as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })?;
        for x in rows {
            let (name, size, modified) = x?;
            let path = format!("{}\\{}", row.path.trim_end_matches('\\'), name);
            kids.push(Item {
                id: None, kind: "file", name, path: Some(path), size, files: 1, newest: modified,
                denied: false, children: None,
            });
        }
    }
    kids.sort_by(|a, b| b.size.cmp(&a.size));
    kids.truncate(limit);
    let shown: i64 = kids.iter().map(|k| k.size).sum();
    let rest = row.size - shown;
    if rest > 0 {
        kids.push(Item {
            id: None, kind: "other",
            name: match row.small_count { 0 => "Smaller items".into(), 1 => "1 smaller file".into(), n => format!("{n} smaller files") },
            path: None, size: rest, files: 0, newest: 0, denied: false, children: None,
        });
    }
    Ok(Item {
        id: Some(row.id), kind: "dir", name: row.name, path: Some(row.path), size: row.size,
        files: row.files, newest: row.newest, denied: row.denied, children: Some(kids),
    })
}

/// `used_on_disk`: when viewing a whole drive, the drive's used space, so what the scan
/// couldn't read (system / protected files) shows up as its own block.
pub fn view(conn: &Connection, scan_id: i64, dir_id: Option<i64>, used_on_disk: Option<i64>) -> rusqlite::Result<Option<View>> {
    let (root, root_dir): (String, i64) =
        conn.query_row("SELECT root, root_dir FROM scans WHERE id = ?1", [scan_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let id = dir_id.unwrap_or(root_dir);
    let Some(row) = dir_row(conn, id)? else { return Ok(None) };

    let mut crumbs = Vec::new();
    let mut p = row.parent;
    while let Some(pid) = p {
        match dir_row(conn, pid)? {
            Some(r) => {
                crumbs.push(Crumb { id: r.id, name: r.name });
                p = r.parent;
            }
            None => break,
        }
    }
    crumbs.reverse();
    crumbs.push(Crumb { id: row.id, name: row.name.clone() });

    let is_root = id == root_dir;
    let mut node = load(conn, row, 1, 120)?;
    if is_root {
        if let Some(used) = used_on_disk {
            let hidden = used - node.size;
            if hidden > 256 * 1024 * 1024 {
                if let Some(k) = node.children.as_mut() {
                    k.push(Item {
                        id: None, kind: "hidden", name: "System & protected files".into(), path: None,
                        size: hidden, files: 0, newest: 0, denied: true, children: None,
                    });
                    k.sort_by(|a, b| b.size.cmp(&a.size));
                }
                node.size += hidden;
            }
        }
    }
    Ok(Some(View { scan_id, root, node, crumbs }))
}
