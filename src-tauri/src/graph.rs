//! Builds the graph view: drives -> folders -> projects / apps / games / services,
//! plus "backed up to" links from the backup config.
//!
//! Sizes come from the latest scan of each drive (one pass over the index). Things that
//! aren't folders on disk (registry entries, launcher manifests) are matched to the index by path.

use crate::{drives, helper};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;
use winreg::enums::*;
use winreg::RegKey;

#[derive(Serialize, Clone)]
pub struct Node {
    pub id: String,
    pub kind: &'static str, // drive | folder | project | app | game | service | backup
    pub label: String,
    pub path: Option<String>,
    pub size: i64,
    pub newest: i64,
    pub scan_id: Option<i64>,
    pub dir_id: Option<i64>,
    pub backed_up: Option<bool>,
    pub backups: Vec<String>,
    pub detail: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct Link {
    pub source: String,
    pub target: String,
    pub kind: &'static str, // lives_on | backed_up_to
}

#[derive(Serialize)]
pub struct GraphData {
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
    pub built_ms: u64,
    pub config_path: String,
}

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct BackupRule {
    pub name: String,
    pub target: String,
    pub sources: Vec<String>,
}

#[derive(Deserialize, Serialize, Default)]
pub struct Config {
    #[serde(default)]
    pub backups: Vec<BackupRule>,
}

pub fn config_path() -> std::path::PathBuf {
    helper::data_dir().join("config.json")
}

pub fn load_config() -> Config {
    std::fs::read(config_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// "c:/Foo/bar/" -> "C:\Foo\bar"; drive roots keep their slash ("C:\").
fn norm(p: &str) -> String {
    let mut s = p.trim().trim_matches('"').replace('/', "\\");
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        s = rest.to_string();
    }
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        let mut c = s.chars();
        let first = c.next().unwrap().to_ascii_uppercase();
        s = format!("{first}{}", c.as_str());
        if s.len() == 2 {
            s.push('\\');
        }
    }
    s
}

fn key(p: &str) -> String {
    norm(p).to_lowercase()
}

fn parent(p: &str) -> Option<String> {
    let p = norm(p);
    if p.len() <= 3 {
        return None;
    }
    let i = p.rfind('\\')?;
    Some(if i <= 2 { p[..3].to_string() } else { p[..i].to_string() })
}

fn last_segment(p: &str) -> String {
    let p = norm(p);
    p.rsplit('\\').next().unwrap_or(&p).to_string()
}

fn under(path_key: &str, root_key: &str) -> bool {
    path_key == root_key
        || (path_key.starts_with(root_key)
            && (root_key.ends_with('\\') || path_key.as_bytes().get(root_key.len()) == Some(&b'\\')))
}

struct Leaf {
    kind: &'static str,
    label: String,
    path: String,
    detail: Option<String>,
    est_size: i64,
}

// ---------------------------------------------------------------- detectors

fn steam_games(out: &mut Vec<Leaf>) {
    let Ok(k) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Valve\Steam") else { return };
    let Ok(steam) = k.get_value::<String, _>("SteamPath") else { return };
    let mut libs = vec![norm(&steam)];
    if let Ok(vdf) = std::fs::read_to_string(Path::new(&norm(&steam)).join(r"steamapps\libraryfolders.vdf")) {
        for line in vdf.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("\"path\"") {
                let v = rest.trim().trim_matches('"').replace("\\\\", "\\");
                libs.push(norm(&v));
            }
        }
    }
    let mut seen = HashSet::new();
    for lib in libs {
        if !seen.insert(key(&lib)) {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(Path::new(&lib).join("steamapps")) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if !(n.starts_with("appmanifest_") && n.ends_with(".acf")) {
                continue;
            }
            let Ok(acf) = std::fs::read_to_string(e.path()) else { continue };
            let field = |f: &str| {
                acf.lines().find_map(|l| {
                    let t = l.trim();
                    t.strip_prefix(&format!("\"{f}\"")).map(|r| r.trim().trim_matches('"').to_string())
                })
            };
            if let (Some(name), Some(dir)) = (field("name"), field("installdir")) {
                if name.starts_with("Steamworks") || name.starts_with("Proton") {
                    continue;
                }
                out.push(Leaf {
                    kind: "game",
                    label: name,
                    path: format!(r"{lib}\steamapps\common\{dir}"),
                    detail: Some("Steam".into()),
                    est_size: 0,
                });
            }
        }
    }
}

fn epic_games(out: &mut Vec<Leaf>) {
    let dir = Path::new(r"C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests");
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if e.path().extension().map(|x| x != "item").unwrap_or(true) {
            continue;
        }
        let Ok(b) = std::fs::read(e.path()) else { continue };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b) else { continue };
        let (Some(name), Some(loc)) = (v["DisplayName"].as_str(), v["InstallLocation"].as_str()) else { continue };
        out.push(Leaf { kind: "game", label: name.into(), path: norm(loc), detail: Some("Epic Games".into()), est_size: 0 });
    }
}

fn ubisoft_games(out: &mut Vec<Leaf>) {
    let Ok(k) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(r"SOFTWARE\WOW6432Node\Ubisoft\Launcher\Installs") else { return };
    for id in k.enum_keys().flatten() {
        let Ok(sub) = k.open_subkey(&id) else { continue };
        let Ok(dir) = sub.get_value::<String, _>("InstallDir") else { continue };
        let p = norm(&dir);
        out.push(Leaf { kind: "game", label: last_segment(&p), path: p, detail: Some("Ubisoft Connect".into()), est_size: 0 });
    }
}

fn xbox_games(out: &mut Vec<Leaf>) {
    for d in drives::list() {
        let Ok(rd) = std::fs::read_dir(format!(r"{}:\XboxGames", d.letter)) else { continue };
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let name = e.file_name().to_string_lossy().to_string();
                if name.eq_ignore_ascii_case("GameSave") {
                    continue;
                }
                out.push(Leaf {
                    kind: "game",
                    label: name,
                    path: norm(&e.path().to_string_lossy()),
                    detail: Some("Xbox app".into()),
                    est_size: 0,
                });
            }
        }
    }
}

fn wsl(out: &mut Vec<Leaf>) {
    let Ok(k) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Lxss") else { return };
    for id in k.enum_keys().flatten() {
        let Ok(sub) = k.open_subkey(&id) else { continue };
        let (Ok(name), Ok(base)) = (sub.get_value::<String, _>("DistributionName"), sub.get_value::<String, _>("BasePath")) else { continue };
        let p = norm(&base);
        let vhd = std::fs::metadata(Path::new(&p).join("ext4.vhdx")).map(|m| m.len() as i64).unwrap_or(0);
        out.push(Leaf { kind: "service", label: format!("WSL · {name}"), path: p, detail: Some("Linux (WSL) disk".into()), est_size: vhd });
    }
}

fn is_gamey(path_key: &str, publisher: &str) -> bool {
    ["\\steamapps\\", "\\epic games\\", "\\epicgames\\", "\\xboxgames\\", "\\games\\", "riot games", "\\battle.net"]
        .iter()
        .any(|m| path_key.contains(m))
        || ["ubisoft", "valve", "riot games", "blizzard", "electronic arts", "rockstar games", "mojang"]
            .iter()
            .any(|m| publisher.to_lowercase().contains(m))
}

fn installed_apps(out: &mut Vec<Leaf>) {
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
            if let Ok(rt) = s.get_value::<String, _>("ReleaseType") {
                if rt.contains("Update") || rt.contains("Hotfix") {
                    continue;
                }
            }
            let Ok(loc) = s.get_value::<String, _>("InstallLocation") else { continue };
            let p = norm(&loc);
            // skip empty / drive-root / bare "Program Files" style locations
            if p.len() < 4 || p.matches('\\').count() < 2 {
                continue;
            }
            let publisher = s.get_value::<String, _>("Publisher").unwrap_or_default();
            let est = s.get_value::<u32, _>("EstimatedSize").map(|kb| kb as i64 * 1024).unwrap_or(0);
            let kind = if is_gamey(&p.to_lowercase(), &publisher) { "game" } else { "app" };
            out.push(Leaf {
                kind,
                label: name,
                path: p,
                detail: if publisher.is_empty() { None } else { Some(publisher) },
                est_size: est,
            });
        }
    }
}

// ---------------------------------------------------------------- index lookups

struct DirInfo {
    id: i64,
    scan: i64,
    path: String,
    size: i64,
    newest: i64,
}

fn latest_scans(conn: &Connection) -> rusqlite::Result<Vec<(i64, String, i64)>> {
    let mut s = conn.prepare("SELECT id, root, root_dir FROM scans")?;
    let r = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    r.collect()
}

fn lookup(conn: &Connection, keys: &HashSet<String>) -> rusqlite::Result<HashMap<String, DirInfo>> {
    let json = serde_json::to_string(&keys.iter().collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into());
    let mut s = conn.prepare(
        "SELECT id, scan_id, path, size, newest FROM dirs WHERE lower(path) IN (SELECT value FROM json_each(?1))",
    )?;
    let rows = s.query_map(params![json], |r| {
        Ok(DirInfo { id: r.get(0)?, scan: r.get(1)?, path: r.get(2)?, size: r.get(3)?, newest: r.get(4)? })
    })?;
    let mut m = HashMap::new();
    for r in rows {
        let r = r?;
        m.insert(r.path.to_lowercase(), r);
    }
    Ok(m)
}

fn projects(conn: &Connection, profile_key: &str) -> rusqlite::Result<Vec<String>> {
    let skip = |k: &str| {
        let rest = &k[profile_key.len()..];
        rest.contains("\\appdata\\") || rest.contains("\\node_modules\\") || rest.split('\\').any(|seg| seg.starts_with('.'))
    };
    // git repositories in the user's folders
    let mut git: Vec<String> = {
        let mut s = conn.prepare("SELECT p.path FROM dirs d JOIN dirs p ON p.id = d.parent_id WHERE d.name = '.git'")?;
        let r = s.query_map([], |r| r.get::<_, String>(0))?;
        r.filter_map(|x| x.ok()).filter(|p| { let k = p.to_lowercase(); under(&k, profile_key) && !skip(&k) }).collect()
    };
    git.sort();
    git.dedup();
    let git_keys: HashSet<String> = git.iter().map(|p| p.to_lowercase()).collect();

    // everything directly inside a "Projects" folder; a folder whose direct children are repos is a category
    let mut cands: Vec<String> = Vec::new();
    let homes = [format!(r"{profile_key}\projects"), format!(r"{profile_key}\documents\projects")];
    let mut s = conn.prepare("SELECT id, path FROM dirs WHERE lower(name) = 'projects'")?;
    let proj_dirs: Vec<(i64, String)> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(|x| x.ok()).collect();
    let mut kids = conn.prepare("SELECT id, path FROM dirs WHERE parent_id = ?1")?;
    for (id, p) in proj_dirs {
        if !homes.contains(&p.to_lowercase()) {
            continue;
        }
        let children: Vec<(i64, String)> = kids.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(|x| x.ok()).collect();
        for (cid, cp) in children {
            let ck = cp.to_lowercase();
            let grand: Vec<String> = kids.query_map([cid], |r| r.get::<_, String>(1))?.filter_map(|x| x.ok()).collect();
            let is_category = !git_keys.contains(&ck) && grand.iter().any(|g| git_keys.contains(&g.to_lowercase()));
            if is_category {
                cands.extend(grand.into_iter().filter(|g| !junk(g)));
            } else if !junk(&cp) {
                cands.push(cp);
            }
        }
    }
    cands.extend(git);
    // keep the outermost: drop anything inside another project
    cands.sort_by_key(|p| p.len());
    let mut keep: Vec<String> = Vec::new();
    for c in cands {
        let ck = c.to_lowercase();
        if !keep.iter().any(|k| under(&ck, &k.to_lowercase())) {
            keep.push(c);
        }
    }
    Ok(keep)
}

/// Folders that are never projects in their own right.
fn junk(p: &str) -> bool {
    let n = last_segment(p).to_lowercase();
    n.starts_with('.')
        || ["node_modules", "venv", "env", "__pycache__", "dist", "build", "out", "target", "bin", "obj"].contains(&n.as_str())
}

// ---------------------------------------------------------------- build

pub fn build(conn: &Connection) -> rusqlite::Result<GraphData> {
    let t0 = Instant::now();
    let cfg = load_config();
    let profile = norm(&std::env::var("USERPROFILE").unwrap_or_default());
    let profile_key = profile.to_lowercase();
    let scans = latest_scans(conn)?;
    let scanned: HashSet<String> = scans.iter().map(|(_, r, _)| r[..1].to_uppercase()).collect();

    // leaves from every detector; earlier detectors win on duplicate paths
    let mut leaves = Vec::new();
    steam_games(&mut leaves);
    epic_games(&mut leaves);
    ubisoft_games(&mut leaves);
    xbox_games(&mut leaves);
    wsl(&mut leaves);
    installed_apps(&mut leaves);
    for p in projects(conn, &profile_key)? {
        leaves.push(Leaf { kind: "project", label: last_segment(&p), path: p, detail: None, est_size: 0 });
    }
    let mut seen = HashSet::new();
    leaves.retain(|l| {
        let k = key(&l.path);
        // only things on drives we've scanned, and each path once
        !l.path.is_empty() && l.path.is_char_boundary(1) && scanned.contains(&l.path[..1].to_uppercase()) && seen.insert(k)
    });

    // top-level folders of each scanned drive
    let mut tops: Vec<(String, i64)> = Vec::new();
    {
        let mut s = conn.prepare("SELECT path, size FROM dirs WHERE parent_id = ?1 ORDER BY size DESC LIMIT 10")?;
        for (_, _, root_dir) in &scans {
            let rows: Vec<(String, i64)> = s.query_map([root_dir], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(|x| x.ok()).collect();
            tops.extend(rows.into_iter().filter(|(_, sz)| *sz >= 1 << 30));
        }
    }

    // every path we need sizes for: leaves, their ancestors, top folders, backup sources/targets
    let mut want: HashSet<String> = HashSet::new();
    fn add_chain(p: &str, want: &mut HashSet<String>) {
        let mut cur = Some(norm(p));
        while let Some(c) = cur {
            if !want.insert(c.to_lowercase()) {
                break;
            }
            cur = parent(&c);
        }
    }
    for l in &leaves {
        add_chain(&l.path, &mut want);
    }
    for (p, _) in &tops {
        add_chain(p, &mut want);
    }
    for b in &cfg.backups {
        add_chain(&b.target, &mut want);
        for s in &b.sources {
            add_chain(s, &mut want);
        }
    }
    let info = lookup(conn, &want)?;

    let mut nodes: Vec<Node> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut links: Vec<Link> = Vec::new();

    // drives
    for d in drives::list() {
        let id = format!("drive:{}", d.letter);
        let label = if d.label.is_empty() { format!("{}: Local disk", d.letter) } else { format!("{}: {}", d.letter, d.label) };
        let root = info.get(&format!("{}:\\", d.letter.to_lowercase()));
        index.insert(id.clone(), nodes.len());
        nodes.push(Node {
            id, kind: "drive", label, path: Some(format!("{}:\\", d.letter)), size: d.total - d.free,
            newest: 0, scan_id: root.map(|r| r.scan), dir_id: root.map(|r| r.id), backed_up: None, backups: vec![],
            detail: Some(format!("{} free of {}", crate::fmt_gb(d.free), crate::fmt_gb(d.total))),
        });
    }

    fn ensure_folder(
        p: &str, info: &HashMap<String, DirInfo>, nodes: &mut Vec<Node>, index: &mut HashMap<String, usize>, links: &mut Vec<Link>,
    ) -> Option<String> {
        let p = norm(p);
        if p.len() <= 3 {
            let id = format!("drive:{}", &p[..1]);
            return index.contains_key(&id).then_some(id);
        }
        let k = p.to_lowercase();
        let id = format!("dir:{k}");
        if index.contains_key(&id) {
            return Some(id);
        }
        let up = parent(&p).and_then(|pp| ensure_folder(&pp, info, nodes, index, links))?;
        let i = info.get(&k);
        index.insert(id.clone(), nodes.len());
        nodes.push(Node {
            id: id.clone(), kind: "folder", label: last_segment(&p), path: Some(p), size: i.map(|x| x.size).unwrap_or(0),
            newest: i.map(|x| x.newest).unwrap_or(0), scan_id: i.map(|x| x.scan), dir_id: i.map(|x| x.id),
            backed_up: None, backups: vec![], detail: None,
        });
        links.push(Link { source: id.clone(), target: up, kind: "lives_on" });
        Some(id)
    }

    // a top folder that is itself a game / app / project is drawn as that, not twice
    let leaf_keys: HashSet<String> = leaves.iter().map(|l| key(&l.path)).collect();
    for (p, _) in &tops {
        if !leaf_keys.contains(&key(p)) {
            ensure_folder(p, &info, &mut nodes, &mut index, &mut links);
        }
    }

    for l in &leaves {
        let k = key(&l.path);
        let i = info.get(&k);
        let size = i.map(|x| x.size).filter(|s| *s > 0).unwrap_or(l.est_size);
        if l.kind == "app" && size < 20 * 1024 * 1024 {
            continue; // tiny apps would just be noise on the map
        }
        let Some(up) = parent(&l.path).and_then(|pp| ensure_folder(&pp, &info, &mut nodes, &mut index, &mut links)) else { continue };
        let id = format!("{}:{k}", l.kind);
        index.insert(id.clone(), nodes.len());
        nodes.push(Node {
            id: id.clone(), kind: l.kind, label: l.label.clone(), path: Some(norm(&l.path)), size,
            newest: i.map(|x| x.newest).unwrap_or(0), scan_id: i.map(|x| x.scan), dir_id: i.map(|x| x.id),
            backed_up: None, backups: vec![], detail: l.detail.clone(),
        });
        links.push(Link { source: id, target: up, kind: "lives_on" });
    }

    // backups
    let mut protected_sources: HashSet<String> = HashSet::new();
    for b in &cfg.backups {
        let id = format!("backup:{}", b.name);
        let t = norm(&b.target);
        let i = info.get(&t.to_lowercase());
        let drive_id = format!("drive:{}", &t[..1].to_uppercase());
        if !index.contains_key(&drive_id) {
            continue;
        }
        index.insert(id.clone(), nodes.len());
        nodes.push(Node {
            id: id.clone(), kind: "backup", label: b.name.clone(), path: Some(t.clone()), size: i.map(|x| x.size).unwrap_or(0),
            newest: i.map(|x| x.newest).unwrap_or(0), scan_id: i.map(|x| x.scan), dir_id: i.map(|x| x.id),
            backed_up: None, backups: vec![], detail: Some(format!("{} sources", b.sources.len())),
        });
        links.push(Link { source: id.clone(), target: drive_id, kind: "lives_on" });
        for s in &b.sources {
            if let Some(src) = ensure_folder(s, &info, &mut nodes, &mut index, &mut links) {
                protected_sources.insert(src.clone());
                links.push(Link { source: src, target: id.clone(), kind: "backed_up_to" });
            }
        }
    }

    // backup coverage for everything with a path
    for n in nodes.iter_mut() {
        if n.kind == "drive" || n.kind == "backup" {
            continue;
        }
        let Some(p) = &n.path else { continue };
        let pk = p.to_lowercase();
        let hits: Vec<String> = cfg
            .backups
            .iter()
            .filter(|b| b.sources.iter().any(|s| under(&pk, &key(s))))
            .map(|b| b.name.clone())
            .collect();
        n.backed_up = Some(!hits.is_empty());
        n.backups = hits;
    }

    // collapse single-child folder chains (keeps the map readable)
    loop {
        let mut child_count: HashMap<&str, usize> = HashMap::new();
        let mut other_links: HashSet<&str> = HashSet::new();
        for l in &links {
            if l.kind == "lives_on" {
                *child_count.entry(l.target.as_str()).or_default() += 1;
            } else {
                other_links.insert(l.source.as_str());
                other_links.insert(l.target.as_str());
            }
        }
        let parent_of: HashMap<&str, &str> =
            links.iter().filter(|l| l.kind == "lives_on").map(|l| (l.source.as_str(), l.target.as_str())).collect();
        let victim = nodes.iter().find(|n| {
            n.kind == "folder"
                && child_count.get(n.id.as_str()).copied().unwrap_or(0) == 1
                && !other_links.contains(n.id.as_str())
                && parent_of.get(n.id.as_str()).map(|p| !p.starts_with("drive:")).unwrap_or(false)
        });
        let Some(v) = victim.map(|n| n.id.clone()) else { break };
        let up = parent_of[v.as_str()].to_string();
        links.retain(|l| !(l.source == v && l.kind == "lives_on"));
        for l in links.iter_mut() {
            if l.target == v {
                l.target = up.clone();
            }
        }
        nodes.retain(|n| n.id != v);
    }

    Ok(GraphData { nodes, links, built_ms: t0.elapsed().as_millis() as u64, config_path: config_path().to_string_lossy().into() })
}
