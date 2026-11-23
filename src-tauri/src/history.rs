//! v0.6 Timeline: what changed on your drives.
//!
//! The index only keeps the latest scan of each drive, so after every scan we keep a small
//! snapshot: every folder of 10 MB or more, down to 7 levels deep (a few thousand rows for C:).
//! One snapshot per drive per day (a later scan the same day replaces it); after two weeks only the
//! last one of each week is kept, and nothing older than a year. Comparing two snapshots shows what
//! grew, shrank, appeared or disappeared, even when it happened slowly over many scans.
//!
//! Things that don't need history (recently changed big files, installs) come straight from the
//! index, the registry and the game launchers, so the Timeline is useful on day one.

use crate::graph::{self, key, last_segment, parent};
use crate::helper::data_dir;
use crate::watch;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::time::UNIX_EPOCH;
use winreg::enums::*;
use winreg::RegKey;

const SNAP_MIN: i64 = 10 << 20; // folders below 10 MB aren't kept in snapshots
const SNAP_DEPTH: usize = 7;
const REPORT_MIN: i64 = 100 << 20; // changes smaller than 100 MB aren't listed
const BIG_FILE: i64 = 50 << 20;

pub fn init(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS snapshots(
           id INTEGER PRIMARY KEY, root TEXT NOT NULL, time INTEGER, scan_id INTEGER, bytes INTEGER, files INTEGER, dirs INTEGER);
         CREATE INDEX IF NOT EXISTS snapshots_root ON snapshots(root, time);
         CREATE TABLE IF NOT EXISTS snap_dirs(snap_id INTEGER, path TEXT, size INTEGER);
         CREATE INDEX IF NOT EXISTS snap_dirs_snap ON snap_dirs(snap_id);",
    )
}

fn depth(path: &str) -> usize {
    let p = path.trim_end_matches('\\');
    if p.len() <= 2 { 0 } else { p.matches('\\').count() }
}

/// Keeps a snapshot of a finished scan (no-op if that scan already has one).
pub fn record(c: &mut Connection, scan_id: i64) -> rusqlite::Result<()> {
    init(c)?;
    if c.query_row("SELECT 1 FROM snapshots WHERE scan_id = ?1", [scan_id], |_| Ok(())).optional()?.is_some() {
        return Ok(());
    }
    let Some((root, started, bytes, files, dirs)) = c
        .query_row("SELECT root, started, bytes, files, dirs FROM scans WHERE id = ?1", [scan_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?))
        })
        .optional()?
    else {
        return Ok(());
    };
    let time = if started > 0 { started } else { crate::cleanup::now() };
    let rows: Vec<(String, i64)> = {
        let mut s = c.prepare("SELECT path, size FROM dirs WHERE scan_id = ?1 AND size >= ?2")?;
        let r = s.query_map(params![scan_id, SNAP_MIN], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        r.filter_map(|x| x.ok()).filter(|(p, _)| depth(p) <= SNAP_DEPTH).collect()
    };
    let tx = c.transaction()?;
    // one snapshot per drive per local day: a later scan the same day replaces it
    let same_day: Vec<i64> = {
        let mut s = tx.prepare(
            "SELECT id FROM snapshots WHERE root = ?1 AND date(time, 'unixepoch', 'localtime') = date(?2, 'unixepoch', 'localtime')",
        )?;
        let r = s.query_map(params![root, time], |r| r.get(0))?;
        r.collect::<Result<_, _>>()?
    };
    for id in same_day {
        drop_snap(&tx, id)?;
    }
    tx.execute(
        "INSERT INTO snapshots(root, time, scan_id, bytes, files, dirs) VALUES (?1,?2,?3,?4,?5,?6)",
        params![root, time, scan_id, bytes, files, dirs],
    )?;
    let snap = tx.last_insert_rowid();
    {
        let mut ins = tx.prepare("INSERT INTO snap_dirs(snap_id, path, size) VALUES (?1,?2,?3)")?;
        for (p, s) in &rows {
            ins.execute(params![snap, p, s])?;
        }
    }
    prune(&tx, &root, time)?;
    tx.commit()
}

fn drop_snap(c: &Connection, id: i64) -> rusqlite::Result<()> {
    c.execute("DELETE FROM snap_dirs WHERE snap_id = ?1", [id])?;
    c.execute("DELETE FROM snapshots WHERE id = ?1", [id])?;
    Ok(())
}

/// Daily for two weeks, then the last snapshot of each week, for up to a year.
fn prune(c: &Connection, root: &str, now: i64) -> rusqlite::Result<()> {
    let snaps: Vec<(i64, i64)> = {
        let mut s = c.prepare("SELECT id, time FROM snapshots WHERE root = ?1 ORDER BY time DESC")?;
        let r = s.query_map([root], |r| Ok((r.get(0)?, r.get(1)?)))?;
        r.collect::<Result<_, _>>()?
    };
    let mut weeks = HashSet::new();
    for (id, t) in snaps {
        let age = now - t;
        if age <= 14 * 86400 {
            continue;
        }
        if age > 366 * 86400 || !weeks.insert(t / (7 * 86400)) {
            drop_snap(c, id)?;
        }
    }
    Ok(())
}

/// Snapshots every scan that doesn't have one yet (first start after updating).
pub fn backfill(c: &mut Connection) {
    let _ = init(c);
    let ids: Vec<i64> = c
        .prepare("SELECT id FROM scans WHERE id NOT IN (SELECT scan_id FROM snapshots)")
        .and_then(|mut s| s.query_map([], |r| r.get(0))?.collect())
        .unwrap_or_default();
    for id in ids {
        let _ = record(c, id);
    }
}

// ---------------------------------------------------------------- report

#[derive(Serialize)]
pub struct Point {
    pub time: i64,
    pub bytes: i64,
}

#[derive(Serialize)]
pub struct DriveHistory {
    pub root: String,
    pub points: Vec<Point>,
    pub since: Option<i64>, // the snapshot the changes are compared against
    pub change: i64,
}

#[derive(Serialize)]
pub struct Change {
    pub path: String,
    pub label: String,
    pub kind: &'static str, // new | grew | shrank | removed
    pub before: i64,
    pub after: i64,
    pub delta: i64,
    pub scan_id: Option<i64>,
    pub dir_id: Option<i64>,
}

#[derive(Serialize)]
pub struct Install {
    pub name: String,
    pub kind: &'static str, // app | game
    pub detail: String,
    pub when: i64,
    pub size: i64,
    pub path: Option<String>,
}

#[derive(Serialize)]
pub struct BigFile {
    pub name: String,
    pub dir: String,
    pub size: i64,
    pub modified: i64,
    pub scan_id: i64,
    pub dir_id: i64,
}

#[derive(Serialize)]
pub struct Timeline {
    pub days: i64,
    pub cutoff: i64,
    pub history_start: Option<i64>,
    pub drives: Vec<DriveHistory>,
    pub changes: Vec<Change>,
    pub installs: Vec<Install>,
    pub files: Vec<BigFile>,
    pub generated: i64,
}

fn load_snap(c: &Connection, id: i64) -> rusqlite::Result<HashMap<String, (String, i64)>> {
    let mut s = c.prepare("SELECT path, size FROM snap_dirs WHERE snap_id = ?1")?;
    let r = s.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    let mut m = HashMap::new();
    for x in r {
        let (p, sz) = x?;
        m.insert(key(&p), (p, sz));
    }
    Ok(m)
}

fn created(path: &str) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.created())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Compares two snapshots of one drive and keeps the most specific folders that explain each change:
/// a folder is listed when it changed by 100 MB or more and no single subfolder accounts for half of it.
fn diff(c: &Connection, base: (i64, i64), latest: i64, out: &mut Vec<Change>) -> rusqlite::Result<i64> {
    let old = load_snap(c, base.0)?;
    let new = load_snap(c, latest)?;
    // a folder missing from the new snapshot may only have shrunk below 10 MB: ask the live index
    let missing: HashSet<String> = old.keys().filter(|k| !new.contains_key(*k)).cloned().collect();
    let live = graph::lookup(c, &missing)?;

    let mut keys: HashSet<&String> = old.keys().collect();
    keys.extend(new.keys());
    let mut delta: HashMap<&str, (i64, i64)> = HashMap::new();
    for k in &keys {
        let a = old.get(*k).map(|x| x.1).unwrap_or(0);
        let b = new.get(*k).map(|x| x.1).or_else(|| live.get(*k).map(|d| d.size)).unwrap_or(0);
        delta.insert(k.as_str(), (a, b));
    }
    let mut kids: HashMap<String, Vec<&str>> = HashMap::new();
    for k in &keys {
        if let Some(p) = parent(k) {
            kids.entry(p.to_lowercase()).or_default().push(k.as_str());
        }
    }
    let mut total = 0;
    for (k, &(a, b)) in &delta {
        let d = b - a;
        if depth(k) == 0 {
            total += d;
            continue;
        }
        if d.abs() < REPORT_MIN {
            continue;
        }
        let explained = kids.get(*k).map_or(false, |cs| {
            cs.iter().any(|ck| {
                let (ca, cb) = delta[ck];
                (cb - ca).abs() * 2 >= d.abs()
            })
        });
        if explained {
            continue;
        }
        let path = new.get(*k).or_else(|| old.get(*k)).map(|x| x.0.clone()).unwrap_or_default();
        let gone = b == 0 && !live.contains_key(*k);
        let kind = if gone {
            "removed"
        } else if a == 0 && created(&path) >= base.1 {
            "new"
        } else if d > 0 {
            "grew"
        } else {
            "shrank"
        };
        out.push(Change { label: last_segment(&path), path, kind, before: a, after: b, delta: d, scan_id: None, dir_id: None });
    }
    Ok(total)
}

fn installs(cutoff: i64) -> Vec<Install> {
    let mut out: Vec<Install> = Vec::new();
    let roots = [
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Uninstall"),
    ];
    for (hive, path) in roots {
        let Ok(k) = RegKey::predef(hive).open_subkey(path) else { continue };
        for id in k.enum_keys().flatten() {
            let Ok(s) = k.open_subkey(&id) else { continue };
            let Ok(name) = s.get_value::<String, _>("DisplayName") else { continue };
            if s.get_value::<u32, _>("SystemComponent").unwrap_or(0) == 1 || s.get_value::<String, _>("ParentKeyName").is_ok() {
                continue;
            }
            let Ok(date) = s.get_value::<String, _>("InstallDate") else { continue };
            let Some(when) = parse_yyyymmdd(&date) else { continue };
            if when < cutoff {
                continue;
            }
            let publisher = s.get_value::<String, _>("Publisher").unwrap_or_default();
            let loc = s.get_value::<String, _>("InstallLocation").ok().map(|l| graph::norm(&l)).filter(|l| l.len() > 3);
            let game = graph::is_gamey(&loc.clone().unwrap_or_default().to_lowercase(), &publisher);
            out.push(Install {
                name,
                kind: if game { "game" } else { "app" },
                detail: publisher,
                when,
                size: s.get_value::<u32, _>("EstimatedSize").map(|kb| kb as i64 * 1024).unwrap_or(0),
                path: loc,
            });
        }
    }
    // games from launchers: the install folder's creation time
    for g in graph::game_leaves() {
        let when = created(&g.2);
        if when >= cutoff {
            out.push(Install { name: g.0, kind: "game", detail: g.1, when, size: 0, path: Some(g.2) });
        }
    }
    let mut seen = HashSet::new();
    out.sort_by(|a, b| b.when.cmp(&a.when));
    out.retain(|i| seen.insert(i.name.to_lowercase()));
    out
}

/// "20261121" -> noon UTC that day (shown as a date only).
fn parse_yyyymmdd(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() != 8 {
        return None;
    }
    let (y, m, d): (i64, i64, i64) = (s[..4].parse().ok()?, s[4..6].parse().ok()?, s[6..].parse().ok()?);
    if !(1990..=2100).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // days from civil (Howard Hinnant)
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146097 + doe - 719468) * 86400 + 12 * 3600)
}

fn big_files(c: &Connection, cutoff: i64) -> rusqlite::Result<Vec<BigFile>> {
    let own = data_dir().to_string_lossy().to_lowercase();
    let mut s = c.prepare(
        "SELECT f.name, d.path, f.size, f.modified, d.scan_id, d.id FROM files f JOIN dirs d ON d.id = f.dir_id
         WHERE f.modified >= ?1 AND f.size >= ?2 ORDER BY f.size DESC LIMIT 400",
    )?;
    let r = s.query_map(params![cutoff, BIG_FILE], |r| {
        Ok(BigFile { name: r.get(0)?, dir: r.get(1)?, size: r.get(2)?, modified: r.get(3)?, scan_id: r.get(4)?, dir_id: r.get(5)? })
    })?;
    // copies made by your backups aren't news, and virtual disks change all the time
    let targets: Vec<String> = graph::load_config().backups.iter().map(|b| key(&b.target)).filter(|t| t.len() > 3).collect();
    let mut per_dir: HashMap<String, usize> = HashMap::new();
    let mut v: Vec<BigFile> = r
        .filter_map(|x| x.ok())
        .filter(|f| {
            let dk = key(&f.dir);
            let n = f.name.to_lowercase();
            !watch::noise(&format!("{dk}\\{n}"), &own)
                && !targets.iter().any(|t| graph::under(&dk, t))
                && ![".vhdx", ".vhd", ".vmdk", ".ost", ".pst"].iter().any(|e| n.ends_with(e))
        })
        .filter(|f| {
            // at most 3 per folder (a game update touches dozens of pak files)
            let n = per_dir.entry(key(&f.dir)).or_default();
            *n += 1;
            *n <= 3
        })
        .collect();
    v.truncate(40);
    Ok(v)
}

pub fn report(c: &mut Connection, days: i64) -> rusqlite::Result<Timeline> {
    backfill(c);
    let now = crate::cleanup::now();
    let cutoff = if days > 0 { now - days * 86400 } else { 0 };
    let history_start: Option<i64> = c.query_row("SELECT MIN(time) FROM snapshots", [], |r| r.get(0))?;

    let scans = crate::db::scans(c)?;
    let mut drives = Vec::new();
    let mut changes = Vec::new();
    for sc in &scans {
        let snaps: Vec<(i64, i64, i64)> = {
            let mut s = c.prepare("SELECT id, time, bytes FROM snapshots WHERE root = ?1 ORDER BY time")?;
            let r = s.query_map([&sc.root], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            r.collect::<Result<_, _>>()?
        };
        let Some(&latest) = snaps.last() else { continue };
        // compare with the newest snapshot from before the cutoff, or the oldest one we have
        let base = snaps.iter().rev().find(|s| s.1 <= cutoff).or(snaps.first()).copied().filter(|b| b.0 != latest.0);
        let mut change = 0;
        if let Some(b) = base {
            diff(c, (b.0, b.1), latest.0, &mut changes)?;
            change = latest.2 - b.2;
        }
        drives.push(DriveHistory {
            root: sc.root.clone(),
            points: snaps.iter().map(|s| Point { time: s.1, bytes: s.2 }).collect(),
            since: base.map(|b| b.1),
            change,
        });
    }
    changes.sort_by(|a, b| b.delta.abs().cmp(&a.delta.abs()));
    changes.truncate(80);
    let want: HashSet<String> = changes.iter().map(|ch| key(&ch.path)).collect();
    let live = graph::lookup(c, &want)?;
    for ch in &mut changes {
        if let Some(d) = live.get(&key(&ch.path)) {
            ch.scan_id = Some(d.scan);
            ch.dir_id = Some(d.id);
        }
    }

    Ok(Timeline {
        days,
        cutoff,
        history_start,
        drives,
        changes,
        installs: installs(cutoff),
        files: big_files(c, cutoff)?,
        generated: now,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates() {
        assert_eq!(super::parse_yyyymmdd("19700101"), None); // out of range
        assert_eq!(super::parse_yyyymmdd("20000301"), Some(951868800 + 12 * 3600));
        assert_eq!(super::parse_yyyymmdd("2026"), None);
    }
}
