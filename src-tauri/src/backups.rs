//! Data for the Backups view: where each backup goes, what feeds it, how much of each
//! drive is protected (by 0 / 1 / 2+ backups), and which of your folders aren't backed up.

use crate::drives;
use crate::graph::{key, last_segment, load_config, lookup, norm, parent, under};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;
use std::time::UNIX_EPOCH;

#[derive(Serialize)]
pub struct Source {
    pub path: String,
    pub label: String,
    pub size: i64,
    pub exists: bool,
    pub newest: i64,
    pub scan_id: Option<i64>,
    pub dir_id: Option<i64>,
}

#[derive(Serialize)]
pub struct Destination {
    pub name: String,
    pub target: String,
    pub drive: String,
    pub drive_label: String,
    pub connected: bool,
    pub target_exists: bool,
    pub drive_free: i64,
    pub drive_total: i64,
    pub stored: i64,
    pub last_activity: i64,
    pub sources: Vec<Source>,
}

#[derive(Serialize)]
pub struct DriveCoverage {
    pub letter: String,
    pub label: String,
    pub used: i64,
    pub twice: i64,
    pub once: i64,
    pub none: i64,
    pub scanned: bool,
}

#[derive(Serialize)]
pub struct Folder {
    pub path: String,
    pub label: String,
    pub size: i64,
    pub newest: i64,
    pub backups: Vec<String>,
    pub hint: Option<String>,
    pub scan_id: Option<i64>,
    pub dir_id: Option<i64>,
}

#[derive(Serialize)]
pub struct Report {
    pub destinations: Vec<Destination>,
    pub coverage: Vec<DriveCoverage>,
    pub folders: Vec<Folder>,
    pub config_path: String,
}

fn mtime(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Best live guess at when a backup last ran: newest timestamp among the target folder,
/// its direct children, and log / status files next to it.
fn last_activity(target: &str) -> i64 {
    let t = Path::new(target);
    let mut best = mtime(t);
    let mut scan = |dir: &Path, only_logs: bool| {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten().take(200) {
                let n = e.file_name().to_string_lossy().to_lowercase();
                if only_logs && !(n.contains("log") || n.contains("last") || n.contains("status")) {
                    continue;
                }
                best = best.max(mtime(&e.path()));
                if only_logs && e.path().is_dir() {
                    if let Ok(rd2) = std::fs::read_dir(e.path()) {
                        for f in rd2.flatten().take(500) {
                            best = best.max(mtime(&f.path()));
                        }
                    }
                }
            }
        }
    };
    scan(t, false);
    if let Some(p) = t.parent() {
        scan(p, true);
    }
    best
}

const SYSTEM: &[&str] = &[
    "windows", "program files", "program files (x86)", "programdata", "users", "system volume information",
    "recovery", "perflogs", "windowsapps", "wpsystem", "wudownloadcache", "config.msi", "msocache", "intel", "amd",
    "nvidia", "inetpub", "drivers", "esd", "onedrivetemp", "$recycle.bin", "$windows.~bt", "$windows.~ws",
    "$winreagent", "$sysreset", "$getcurrent", "boot", "documents and settings", "msys64", "appdata",
];

fn hint_for(path_key: &str, label: &str) -> Option<String> {
    let l = label.to_lowercase();
    if ["\\steamlibrary", "\\epicgames", "\\epic games", "\\xboxgames", "\\games", "\\steamapps", "riot games"]
        .iter()
        .any(|m| path_key.contains(m))
        || l.contains("rainbow six")
    {
        return Some("Games · can re-download".into());
    }
    match l.as_str() {
        "downloads" => Some("Downloads · usually re-downloadable".into()),
        "desktop" => Some("Desktop".into()),
        "pictures" | "photos" => Some("Photos".into()),
        "videos" => Some("Videos".into()),
        "music" => Some("Music".into()),
        ".rustup" | ".cargo" | ".cache" | ".npm" | ".nuget" | ".gradle" | ".m2" | ".vscode" | ".cursor" | ".bun"
        | ".docker" | ".local" | ".rokit" | ".pyenv" | ".conda" | ".ollama" | ".deno" | ".pnpm-store" | ".yarn" => {
            Some("Tools & caches · reinstallable".into())
        }
        _ if l.starts_with('.') => Some("App data".into()),
        _ => None,
    }
}

pub fn report(conn: &Connection) -> rusqlite::Result<Report> {
    let cfg = load_config();
    let drive_list = drives::list();
    let scanned_roots: HashSet<String> = {
        let mut s = conn.prepare("SELECT root FROM scans")?;
        let r = s.query_map([], |r| r.get::<_, String>(0))?;
        r.filter_map(|x| x.ok()).map(|r| r[..1].to_uppercase()).collect()
    };
    let dest_drives: HashSet<String> = cfg.backups.iter().map(|b| norm(&b.target)[..1].to_uppercase()).collect();

    // candidate "your folders": top folders of each non-destination drive + user profile folders
    let profile = norm(&std::env::var("USERPROFILE").unwrap_or_default());
    let profile_key = profile.to_lowercase();
    let mut candidates: Vec<String> = Vec::new();
    {
        let mut s = conn.prepare(
            "SELECT d.path FROM dirs d JOIN scans s ON d.parent_id = s.root_dir AND d.scan_id = s.id",
        )?;
        let tops: Vec<String> = s.query_map([], |r| r.get::<_, String>(0))?.filter_map(|x| x.ok()).collect();
        for t in tops {
            let l = last_segment(&t).to_lowercase();
            if dest_drives.contains(&t[..1].to_uppercase()) || SYSTEM.contains(&l.as_str()) || l.starts_with('$') {
                continue;
            }
            candidates.push(t);
        }
        if let Ok(rd) = std::fs::read_dir(&profile) {
            for e in rd.flatten() {
                if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let n = e.file_name().to_string_lossy().to_string();
                let nl = n.to_lowercase();
                if SYSTEM.contains(&nl.as_str()) || nl.starts_with("ntuser") || nl == "application data" || nl == "local settings"
                    || nl == "cookies" || nl == "nethood" || nl == "printhood" || nl == "recent" || nl == "sendto"
                    || nl == "start menu" || nl == "templates" || nl == "my documents" || nl == "searches" || nl == "links"
                    || nl == "contacts" || nl == "favorites" || nl == "saved games" || nl == "3d objects"
                {
                    continue;
                }
                candidates.push(format!("{profile}\\{n}"));
            }
        }
    }

    // sizes for everything we talk about
    let mut want: HashSet<String> = candidates.iter().map(|c| key(c)).collect();
    for b in &cfg.backups {
        want.insert(key(&b.target));
        for s in &b.sources {
            want.insert(key(s));
        }
    }
    let info = lookup(conn, &want)?;

    // destinations
    let mut destinations = Vec::new();
    for b in &cfg.backups {
        let t = norm(&b.target);
        let letter = t[..1].to_uppercase();
        let d = drive_list.iter().find(|d| d.letter == letter);
        let ti = info.get(&key(&t));
        let target_exists = Path::new(&t).exists();
        let sources = b
            .sources
            .iter()
            .map(|s| {
                let p = norm(s);
                let i = info.get(&key(&p));
                Source {
                    label: last_segment(&p),
                    exists: Path::new(&p).exists(),
                    size: i.map(|x| x.size).unwrap_or(0),
                    newest: i.map(|x| x.newest).unwrap_or(0),
                    scan_id: i.map(|x| x.scan),
                    dir_id: i.map(|x| x.id),
                    path: p,
                }
            })
            .collect();
        destinations.push(Destination {
            name: b.name.clone(),
            drive: letter.clone(),
            drive_label: d.map(|d| d.label.clone()).unwrap_or_default(),
            connected: d.is_some(),
            target_exists,
            drive_free: d.map(|d| d.free).unwrap_or(0),
            drive_total: d.map(|d| d.total).unwrap_or(0),
            stored: ti.map(|x| x.size).unwrap_or(0),
            last_activity: if target_exists { last_activity(&t) } else { 0 },
            target: t,
            sources,
        });
    }

    // coverage per drive: how many bytes are protected by 0 / 1 / 2+ backups.
    // Each source path's "own" bytes = its size minus nested source paths below it.
    let mut coverage = Vec::new();
    for d in &drive_list {
        if dest_drives.contains(&d.letter) || d.kind != "fixed" {
            continue;
        }
        let used = d.total - d.free;
        let mut paths: Vec<String> = Vec::new();
        for b in &cfg.backups {
            for s in &b.sources {
                let k = key(s);
                if k.starts_with(&d.letter.to_lowercase()) && !paths.contains(&k) {
                    paths.push(k);
                }
            }
        }
        let count = |p: &str| {
            cfg.backups.iter().filter(|b| b.sources.iter().any(|s| under(p, &key(s)))).count()
        };
        let size = |p: &str| info.get(p).map(|x| x.size).unwrap_or(0);
        let (mut once, mut twice) = (0i64, 0i64);
        for p in &paths {
            // nearest nested sources directly under p
            let nested: Vec<&String> = paths
                .iter()
                .filter(|q| *q != p && under(q, p))
                .filter(|q| !paths.iter().any(|m| m != p && *m != **q && under(m, p) && under(q, m)))
                .collect();
            let own = (size(p) - nested.iter().map(|q| size(q)).sum::<i64>()).max(0);
            match count(p) {
                0 => {}
                1 => once += own,
                _ => twice += own,
            }
        }
        coverage.push(DriveCoverage {
            letter: d.letter.clone(),
            label: d.label.clone(),
            used,
            twice,
            once,
            none: (used - once - twice).max(0),
            scanned: scanned_roots.contains(&d.letter),
        });
    }

    // your folders with their backup status
    let mut seen = HashSet::new();
    let mut folders: Vec<Folder> = candidates
        .into_iter()
        .filter(|c| seen.insert(key(c)))
        .filter_map(|c| {
            let k = key(&c);
            let i = info.get(&k)?;
            if i.size < 50 * 1024 * 1024 {
                return None; // tiny folders aren't worth listing
            }
            let label = last_segment(&c);
            let backups: Vec<String> = cfg
                .backups
                .iter()
                .filter(|b| b.sources.iter().any(|s| under(&k, &key(s))))
                .map(|b| b.name.clone())
                .collect();
            // a folder that only *contains* backed-up parts is partly covered
            let partial = backups.is_empty() && cfg.backups.iter().any(|b| b.sources.iter().any(|s| under(&key(s), &k)));
            let mut hint = hint_for(&k, &label);
            if partial {
                hint = Some(match hint { Some(h) => format!("{h} · partly backed up"), None => "Partly backed up".into() });
            }
            Some(Folder { path: c, label, size: i.size, newest: i.newest, backups, hint, scan_id: Some(i.scan), dir_id: Some(i.id) })
        })
        .collect();
    folders.sort_by(|a, b| b.size.cmp(&a.size));
    let _ = parent; // (kept for future per-folder drill-down)
    let _ = profile_key;

    Ok(Report { destinations, coverage, folders, config_path: crate::graph::config_path().to_string_lossy().into() })
}
