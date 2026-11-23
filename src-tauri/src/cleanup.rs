//! v0.3 Cleanup: finds things you haven't used in a while, and archives / deletes /
//! restores them safely.
//!
//! Safety rules (from the design doc):
//!  - Archive = zip -> verify every file (count, bytes, CRC) -> originals to the Recycle Bin
//!    -> log it. Nothing is deleted if any step before it fails.
//!  - Delete = Recycle Bin only (restorable from here or the Recycle Bin).
//!  - Never touches system folders, program folders, AppData, dot-folders or drive roots.
//!  - Apps are never removed here: "Uninstall" opens Windows' own uninstaller page.

use crate::graph::{key, last_segment, load_config, lookup, norm, under};
use crate::helper::data_dir;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use winreg::enums::*;
use winreg::RegKey;

const DAY: i64 = 86_400;

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn ymd(ts: i64) -> String {
    // civil-from-days (Howard Hinnant)
    let z = ts.div_euclid(DAY) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

fn mtime(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ------------------------------------------------------------------ state

#[derive(Serialize, Deserialize, Clone)]
pub struct Action {
    pub id: String,
    pub time: i64,
    pub kind: String, // "archive" | "delete"
    pub name: String,
    pub item_kind: String, // project | download | game | ...
    pub from: String,
    pub to: Option<String>, // zip path for archives
    pub files: u64,
    pub bytes: u64,
    pub zip_bytes: u64,
    pub verified: bool,
    pub restored: Option<i64>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct State {
    #[serde(default)]
    pub snoozed: HashMap<String, i64>, // path key -> until
    #[serde(default)]
    pub never: Vec<String>,
    #[serde(default)]
    pub actions: Vec<Action>,
}

fn state_path() -> PathBuf {
    if let Ok(p) = std::env::var("CARTOGRAPHER_STATE_FILE") {
        return PathBuf::from(p); // self-test only: never touches your real log
    }
    data_dir().join("cleanup.json")
}

pub fn load_state() -> State {
    std::fs::read(state_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_state(s: &State) -> Result<(), String> {
    std::fs::create_dir_all(data_dir()).map_err(|e| e.to_string())?;
    let tmp = state_path().with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(tmp, state_path()).map_err(|e| e.to_string())
}

/// Where archives go: config.json "archive_dir", else an "Archive" folder at the root of a
/// non-system drive if one exists, else <profile>\Archive.
pub fn archive_dir() -> String {
    if let Ok(d) = std::env::var("CARTOGRAPHER_ARCHIVE_DIR") {
        return norm(&d); // self-test only
    }
    if let Some(d) = load_config_archive() {
        return norm(&d);
    }
    for d in crate::drives::list() {
        if d.letter != "C" && d.kind == "fixed" && Path::new(&format!("{}:\\Archive", d.letter)).is_dir() {
            return format!("{}:\\Archive", d.letter);
        }
    }
    format!("{}\\Archive", norm(&std::env::var("USERPROFILE").unwrap_or_default()))
}

fn load_config_archive() -> Option<String> {
    let b = std::fs::read(crate::graph::config_path()).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&b).ok()?;
    v.get("archive_dir")?.as_str().map(|s| s.to_string())
}

// ------------------------------------------------------------------ safety

const BLOCKED: &[&str] = &[
    "windows", "program files", "program files (x86)", "programdata", "system volume information", "recovery",
    "$recycle.bin", "appdata", ".ssh", "boot", "perflogs",
];

/// Only things in your own folders or on data drives, never system / program / app-data paths.
pub fn allowed(path: &str) -> Result<(), String> {
    let p = norm(path);
    let k = p.to_lowercase();
    if p.len() <= 3 {
        return Err("Won't touch a whole drive".into());
    }
    if !Path::new(&p).exists() {
        return Err("It's no longer there".into());
    }
    for seg in k.split('\\').skip(1) {
        if BLOCKED.contains(&seg) {
            return Err(format!("Won't touch anything inside \"{seg}\""));
        }
    }
    let profile = key(&std::env::var("USERPROFILE").unwrap_or_default());
    if k == profile || k == format!("{profile}\\documents") || k == format!("{profile}\\desktop") || k == format!("{profile}\\downloads") {
        return Err("Won't touch a whole user folder".into());
    }
    if under(&k, &profile) {
        let rest = &k[profile.len()..];
        if rest.split('\\').filter(|s| !s.is_empty()).next().map(|s| s.starts_with('.')).unwrap_or(false) {
            return Err("Won't touch app settings folders".into());
        }
    } else if k.starts_with("c:") {
        return Err("Outside your user folder on C:".into());
    }
    Ok(())
}

// ------------------------------------------------------------------ usage signals

/// exe path (lowercase) -> last run time, from Windows' UserAssist launch history.
fn userassist() -> Vec<(String, i64)> {
    let mut out = Vec::new();
    let known: HashMap<&str, String> = HashMap::from([
        ("{6d809377-6af0-444b-8957-a3773f02200e}", std::env::var("ProgramW6432").or_else(|_| std::env::var("ProgramFiles")).unwrap_or_default()),
        ("{7c5a40ef-a0fb-4bfc-874a-c0f2e0b9fa8e}", std::env::var("ProgramFiles(x86)").unwrap_or_default()),
        ("{f38bf404-1d43-42f2-9305-67de0b28fc23}", std::env::var("WINDIR").unwrap_or_default()),
        ("{1ac14e77-02e7-4e5d-b744-2eb1ae5198b7}", format!("{}\\System32", std::env::var("WINDIR").unwrap_or_default())),
        ("{d65231b0-b2f1-4857-a4ce-a8e7c6ea7d27}", format!("{}\\SysWOW64", std::env::var("WINDIR").unwrap_or_default())),
    ]);
    let Ok(ua) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist") else { return out };
    for g in ua.enum_keys().flatten() {
        let Ok(count) = ua.open_subkey(format!("{g}\\Count")) else { continue };
        for (name, val) in count.enum_values().flatten() {
            if val.bytes.len() < 68 {
                continue;
            }
            let ft = u64::from_le_bytes(val.bytes[60..68].try_into().unwrap());
            if ft == 0 {
                continue;
            }
            let t = (ft / 10_000_000) as i64 - 11_644_473_600;
            let dec: String = name
                .chars()
                .map(|c| match c {
                    'a'..='z' => (((c as u8 - b'a' + 13) % 26) + b'a') as char,
                    'A'..='Z' => (((c as u8 - b'A' + 13) % 26) + b'A') as char,
                    _ => c,
                })
                .collect();
            let mut p = dec.to_lowercase();
            if p.starts_with('{') {
                if let Some(end) = p.find('}') {
                    if let Some(base) = known.get(&p[..=end]) {
                        p = format!("{}{}", base.to_lowercase(), &p[end + 1..]);
                    }
                }
            }
            if p.len() > 3 && p.as_bytes()[1] == b':' {
                out.push((p, t));
            }
        }
    }
    out
}

/// Steam install dir (lowercase) -> LastPlayed
fn steam_last_played() -> HashMap<String, i64> {
    let mut m = HashMap::new();
    let Ok(k) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Valve\Steam") else { return m };
    let Ok(steam) = k.get_value::<String, _>("SteamPath") else { return m };
    let mut libs = vec![norm(&steam)];
    if let Ok(vdf) = std::fs::read_to_string(Path::new(&norm(&steam)).join(r"steamapps\libraryfolders.vdf")) {
        for line in vdf.lines() {
            if let Some(rest) = line.trim().strip_prefix("\"path\"") {
                libs.push(norm(&rest.trim().trim_matches('"').replace("\\\\", "\\")));
            }
        }
    }
    for lib in libs {
        let Ok(rd) = std::fs::read_dir(Path::new(&lib).join("steamapps")) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if !(n.starts_with("appmanifest_") && n.ends_with(".acf")) {
                continue;
            }
            let Ok(acf) = std::fs::read_to_string(e.path()) else { continue };
            let field = |f: &str| {
                acf.lines().find_map(|l| l.trim().strip_prefix(&format!("\"{f}\"")).map(|r| r.trim().trim_matches('"').to_string()))
            };
            if let (Some(dir), Some(lp)) = (field("installdir"), field("LastPlayed")) {
                if let Ok(t) = lp.parse::<i64>() {
                    m.insert(format!(r"{lib}\steamapps\common\{dir}").to_lowercase(), t);
                }
            }
        }
    }
    m
}

// ------------------------------------------------------------------ suggestions

#[derive(Serialize, Clone)]
pub struct Suggestion {
    pub key: String,
    pub kind: String, // project | game | app | download
    pub name: String,
    pub path: String,
    pub size: i64,
    pub last_used: i64,
    pub last_used_how: String,
    pub action: String, // archive | delete | uninstall
    pub reason: String,
    pub backed_up: Vec<String>,
    pub scan_id: Option<i64>,
    pub dir_id: Option<i64>,
}

#[derive(Serialize)]
pub struct Report {
    pub suggestions: Vec<Suggestion>,
    pub actions: Vec<Action>,
    pub archive_dir: String,
    pub archive_backed_up: Vec<String>,
    pub hidden: usize,
}

fn fmt_age(secs: i64) -> String {
    let d = secs / DAY;
    if d < 60 { format!("{d} days") } else if d < 730 { format!("{} months", d / 30) } else { format!("{:.1} years", d as f64 / 365.0) }
}

pub fn report(conn: &Connection) -> rusqlite::Result<Report> {
    let st = load_state();
    let cfg = load_config();
    let t = now();
    let covered = |k: &str| -> Vec<String> {
        cfg.backups.iter().filter(|b| b.sources.iter().any(|s| under(k, &key(s)))).map(|b| b.name.clone()).collect()
    };

    let graph = crate::graph::build(conn)?;
    let ua = userassist();
    let steam = steam_last_played();
    let mut out: Vec<Suggestion> = Vec::new();

    for n in &graph.nodes {
        let Some(path) = &n.path else { continue };
        let k = key(path);
        match n.kind {
            "project" => {
                // last activity: newest file in the folder, or the last git operation
                let git = mtime(&Path::new(path).join(".git").join("logs").join("HEAD")).max(mtime(&Path::new(path).join(".git").join("index")));
                let last = n.newest.max(git);
                if last == 0 || t - last < 180 * DAY || n.size < 50 * 1024 * 1024 {
                    continue;
                }
                out.push(Suggestion {
                    key: k.clone(), kind: "project".into(), name: n.label.clone(), path: path.clone(), size: n.size,
                    last_used: last, last_used_how: if git >= n.newest && git > 0 { "last git activity".into() } else { "newest file change".into() },
                    action: "archive".into(),
                    reason: format!("Untouched for {}", fmt_age(t - last)),
                    backed_up: covered(&k), scan_id: n.scan_id, dir_id: n.dir_id,
                });
            }
            "game" | "app" => {
                let mut last = ua.iter().filter(|(p, _)| under(p, &k)).map(|(_, t)| *t).max().unwrap_or(0);
                let mut how = "Windows launch history".to_string();
                if let Some(lp) = steam.get(&k) {
                    if *lp > last {
                        last = *lp;
                        how = "Steam".into();
                    }
                }
                let limit = if n.kind == "game" { 180 } else { 365 };
                let min = if n.kind == "game" { 1 << 30 } else { 200 * 1024 * 1024 };
                if last == 0 || t - last < limit * DAY || n.size < min {
                    continue; // unknown or recently used: never guess
                }
                out.push(Suggestion {
                    key: k.clone(), kind: n.kind.into(), name: n.label.clone(), path: path.clone(), size: n.size,
                    last_used: last, last_used_how: how, action: "uninstall".into(),
                    reason: if n.kind == "game" {
                        format!("Not played for {} · can re-download", fmt_age(t - last))
                    } else {
                        format!("Not opened for {}", fmt_age(t - last))
                    },
                    backed_up: vec![], scan_id: n.scan_id, dir_id: n.dir_id,
                });
            }
            _ => {}
        }
    }

    // Downloads older than 60 days
    let downloads = format!("{}\\Downloads", norm(&std::env::var("USERPROFILE").unwrap_or_default()));
    if let Ok(rd) = std::fs::read_dir(&downloads) {
        let mut dirs_wanted = HashSet::new();
        let mut entries = Vec::new();
        for e in rd.flatten() {
            let p = norm(&e.path().to_string_lossy());
            let Ok(md) = e.metadata() else { continue };
            if md.file_attributes() & 0x2 != 0 {
                continue; // hidden (desktop.ini etc.)
            }
            if md.is_dir() {
                dirs_wanted.insert(key(&p));
            }
            entries.push((p, md));
        }
        let info = lookup(conn, &dirs_wanted)?;
        for (p, md) in entries {
            let k = key(&p);
            let (size, last) = if md.is_dir() {
                let i = info.get(&k);
                (i.map(|x| x.size).unwrap_or(0), i.map(|x| x.newest).unwrap_or(mtime(Path::new(&p))))
            } else {
                (md.len() as i64, mtime(Path::new(&p)))
            };
            if t - last < 60 * DAY || size < 10 * 1024 * 1024 {
                continue;
            }
            let ext = Path::new(&p).extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            let installer = ["exe", "msi", "msix", "appx", "iso"].contains(&ext.as_str());
            out.push(Suggestion {
                key: k.clone(), kind: "download".into(), name: last_segment(&p), path: p.clone(), size,
                last_used: last, last_used_how: "date downloaded / changed".into(),
                action: if md.is_dir() || ["zip", "7z", "rar"].contains(&ext.as_str()) && !installer { "archive".into() } else { "delete".into() },
                reason: if installer { format!("Installer from {} ago", fmt_age(t - last)) } else { format!("In Downloads for {}", fmt_age(t - last)) },
                backed_up: covered(&k), scan_id: None, dir_id: None,
            });
        }
    }

    // drop snoozed / never / already-archived
    let gone: HashSet<String> = st.actions.iter().filter(|a| a.restored.is_none()).map(|a| key(&a.from)).collect();
    let before = out.len();
    out.retain(|s| {
        !st.never.contains(&s.key) && st.snoozed.get(&s.key).map(|u| *u < t).unwrap_or(true) && !gone.contains(&s.key)
    });
    let hidden = before - out.len();
    out.sort_by(|a, b| b.size.cmp(&a.size));

    let ad = archive_dir();
    let archive_backed_up = covered(&key(&ad));
    let mut actions = st.actions.clone();
    actions.reverse();
    Ok(Report { suggestions: out, actions, archive_dir: ad, archive_backed_up, hidden })
}

pub fn snooze(key_: &str, days: i64) -> Result<(), String> {
    let mut s = load_state();
    s.snoozed.insert(key_.to_string(), now() + days * DAY);
    save_state(&s)
}

pub fn never(key_: &str) -> Result<(), String> {
    let mut s = load_state();
    if !s.never.iter().any(|k| k == key_) {
        s.never.push(key_.to_string());
    }
    save_state(&s)
}

pub fn unhide_all() -> Result<(), String> {
    let mut s = load_state();
    s.snoozed.clear();
    s.never.clear();
    save_state(&s)
}

// ------------------------------------------------------------------ archive / restore / delete

pub struct Progress<'a> {
    pub report: &'a dyn Fn(&str, u64, u64),
}

fn is_reparse(md: &std::fs::Metadata) -> bool {
    md.file_attributes() & 0x400 != 0
}

fn unique(p: PathBuf) -> PathBuf {
    if !p.exists() {
        return p;
    }
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = p.extension().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    for i in 2..1000 {
        let c = p.with_file_name(format!("{stem} ({i}).{ext}"));
        if !c.exists() {
            return c;
        }
    }
    p
}

/// Zip `path` into the archive folder, verify it, then send the original to the Recycle Bin.
pub fn archive(path: &str, item_kind: &str, prog: &Progress) -> Result<Action, String> {
    use zip::write::SimpleFileOptions;
    allowed(path)?;
    let src = PathBuf::from(norm(path));
    let name = last_segment(&norm(path));
    let folder = match item_kind {
        "project" => "Projects",
        "download" => "Downloads",
        _ => "Other",
    };
    let dir = PathBuf::from(archive_dir()).join(folder);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Can't create {}: {e}", dir.display()))?;
    let zip_path = unique(dir.join(format!("{name} ({}).zip", ymd(now()))));

    // 1. list everything (skipping junctions / links so nothing outside is pulled in)
    (prog.report)("listing files", 0, 0);
    let mut files: Vec<(PathBuf, String, u64)> = Vec::new();
    let mut dirs: Vec<String> = Vec::new();
    let is_file = src.is_file();
    if is_file {
        let len = std::fs::metadata(&src).map_err(|e| e.to_string())?.len();
        files.push((src.clone(), name.clone(), len));
    } else {
        let walker = walkdir::WalkDir::new(&src).follow_links(false).into_iter().filter_entry(|e| {
            e.depth() == 0 || e.metadata().map(|m| !is_reparse(&m)).unwrap_or(false)
        });
        for e in walker {
            let e = e.map_err(|e| format!("Couldn't read part of the folder: {e}"))?;
            if e.depth() == 0 {
                continue;
            }
            let rel = e.path().strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
            if e.file_type().is_dir() {
                dirs.push(format!("{rel}/"));
            } else if e.file_type().is_file() {
                files.push((e.path().to_path_buf(), rel, e.metadata().map(|m| m.len()).unwrap_or(0)));
            }
        }
    }
    let total: u64 = files.iter().map(|f| f.2).sum();

    // 2. write the zip, using every CPU core: files are split into balanced batches (biggest
    //    first, each to the lightest batch), every core compresses its batch into its own part
    //    file at the same time, then the parts are joined into the final zip by copying the
    //    already-compressed data (nothing is compressed twice).
    let threads = std::env::var("CARTOGRAPHER_ZIP_THREADS") // testing only: force a core count
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4))
        .clamp(1, 32);
    let mut order: Vec<usize> = (0..files.len()).collect();
    order.sort_by(|&x, &y| files[y].2.cmp(&files[x].2));
    let mut batches: Vec<(u64, Vec<usize>)> = vec![(0, Vec::new()); threads.min(files.len().max(1))];
    for i in order {
        let lightest = batches.iter_mut().min_by_key(|b| b.0).unwrap();
        lightest.0 += files[i].2.max(1);
        lightest.1.push(i);
    }
    let parts: Vec<PathBuf> = (0..batches.len()).map(|k| zip_path.with_extension(format!("zip.part{k}"))).collect();
    let cleanup_parts = |parts: &[PathBuf]| {
        for p in parts {
            let _ = std::fs::remove_file(p);
        }
    };
    let done = std::sync::atomic::AtomicU64::new(0);
    let failed = std::sync::Mutex::new(None::<String>);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(true);
    std::thread::scope(|sc| {
        for (k, (_, idx)) in batches.iter().enumerate() {
            let (part, files, done, failed, stop) = (&parts[k], &files, &done, &failed, &stop);
            sc.spawn(move || {
                let r = (|| -> Result<(), String> {
                    let f = File::create(part).map_err(|e| e.to_string())?;
                    let mut zw = zip::ZipWriter::new(io::BufWriter::with_capacity(1 << 20, f));
                    let mut buf = vec![0u8; 1 << 20];
                    for &i in idx {
                        if stop.load(std::sync::atomic::Ordering::Relaxed) {
                            return Ok(());
                        }
                        let (p, rel, _) = &files[i];
                        zw.start_file(rel.as_str(), opts).map_err(|e| e.to_string())?;
                        let mut r = File::open(p).map_err(|e| format!("Couldn't open {rel}: {e} (is it open in another app?)"))?;
                        loop {
                            let n = r.read(&mut buf).map_err(|e| format!("Couldn't read {rel}: {e}"))?;
                            if n == 0 {
                                break;
                            }
                            zw.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                            done.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    let mut inner = zw.finish().map_err(|e| e.to_string())?;
                    inner.flush().map_err(|e| e.to_string())?;
                    Ok(())
                })();
                if let Err(e) = r {
                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    failed.lock().unwrap().get_or_insert(e);
                }
            });
        }
        // progress from this thread while the workers run
        loop {
            let d = done.load(std::sync::atomic::Ordering::Relaxed);
            (prog.report)(&format!("zipping on {} cores", batches.len()), d.min(total), total);
            if d >= total || stop.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    });
    if let Some(e) = failed.into_inner().unwrap() {
        cleanup_parts(&parts);
        let _ = std::fs::remove_file(&zip_path);
        return Err(format!("Archiving stopped, nothing was removed. {e}"));
    }
    (prog.report)("joining the parts", total, total);
    let result = (|| -> Result<(), String> {
        let f = File::create(&zip_path).map_err(|e| e.to_string())?;
        let mut zw = zip::ZipWriter::new(io::BufWriter::with_capacity(1 << 20, f));
        for d in &dirs {
            zw.add_directory(d.as_str(), opts).map_err(|e| e.to_string())?;
        }
        for part in &parts {
            let za = zip::ZipArchive::new(File::open(part).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            zw.merge_archive(za).map_err(|e| e.to_string())?;
        }
        let mut inner = zw.finish().map_err(|e| e.to_string())?;
        inner.flush().map_err(|e| e.to_string())?;
        Ok(())
    })();
    cleanup_parts(&parts);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&zip_path);
        return Err(format!("Archiving stopped, nothing was removed. {e}"));
    }

    // 3. verify (also on every core): every file present, same sizes, CRC checked by reading
    //    each one back. Each core opens the zip and checks its share of the entries.
    (prog.report)("checking the zip", 0, total);
    let verify = (|| -> Result<(), String> {
        let n_entries = zip::ZipArchive::new(File::open(&zip_path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?.len();
        let want: HashMap<&str, u64> = files.iter().map(|(_, r, l)| (r.as_str(), *l)).collect();
        let seen = std::sync::atomic::AtomicUsize::new(0);
        let checked = std::sync::atomic::AtomicU64::new(0);
        let bad = std::sync::Mutex::new(None::<String>);
        let stop = std::sync::atomic::AtomicBool::new(false);
        let workers = threads.min(n_entries.max(1));
        std::thread::scope(|sc| {
            for w in 0..workers {
                let (want, seen, checked, bad, stop, zp) = (&want, &seen, &checked, &bad, &stop, &zip_path);
                sc.spawn(move || {
                    let r = (|| -> Result<(), String> {
                        let mut za = zip::ZipArchive::new(File::open(zp).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                        let mut i = w;
                        while i < n_entries {
                            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                                return Ok(());
                            }
                            let mut zf = za.by_index(i).map_err(|e| e.to_string())?;
                            if !zf.is_dir() {
                                let name = zf.name().to_string();
                                let expect = *want.get(name.as_str()).ok_or_else(|| format!("unexpected entry {name}"))?;
                                let n = io::copy(&mut zf, &mut io::sink()).map_err(|e| format!("{name} failed its check: {e}"))?;
                                if n != expect {
                                    return Err(format!("{name}: {n} bytes in the zip, {expect} on disk"));
                                }
                                seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                checked.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                            }
                            i += workers;
                        }
                        Ok(())
                    })();
                    if let Err(e) = r {
                        stop.store(true, std::sync::atomic::Ordering::Relaxed);
                        bad.lock().unwrap().get_or_insert(e);
                    }
                });
            }
            loop {
                let c = checked.load(std::sync::atomic::Ordering::Relaxed);
                (prog.report)("checking the zip", c.min(total), total);
                if c >= total || stop.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        });
        if let Some(e) = bad.into_inner().unwrap() {
            return Err(e);
        }
        let seen = seen.into_inner();
        if seen != files.len() {
            return Err(format!("{seen} files in the zip, {} on disk", files.len()));
        }
        Ok(())
    })();
    if let Err(e) = verify {
        let _ = std::fs::remove_file(&zip_path);
        return Err(format!("The zip didn't check out, so nothing was removed. {e}"));
    }

    // 4. originals to the Recycle Bin
    (prog.report)("moving originals to the Recycle Bin", total, total);
    trash::delete(&src).map_err(|e| format!("Zip saved and verified at {}, but the originals couldn't be moved to the Recycle Bin: {e}", zip_path.display()))?;

    let zip_bytes = std::fs::metadata(&zip_path).map(|m| m.len()).unwrap_or(0);
    let a = Action {
        id: format!("{:x}", now() * 1000 + (zip_bytes % 1000) as i64),
        time: now(), kind: "archive".into(), name, item_kind: item_kind.into(),
        from: norm(path), to: Some(zip_path.to_string_lossy().to_string()),
        files: files.len() as u64, bytes: total, zip_bytes, verified: true, restored: None,
    };
    let mut s = load_state();
    s.actions.push(a.clone());
    save_state(&s)?;
    Ok(a)
}

/// Send a file / folder to the Recycle Bin and log it.
pub fn delete(path: &str, item_kind: &str) -> Result<Action, String> {
    allowed(path)?;
    let p = norm(path);
    let bytes = if Path::new(&p).is_file() { std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) } else { 0 };
    trash::delete(&p).map_err(|e| format!("Couldn't move it to the Recycle Bin: {e}"))?;
    let a = Action {
        id: format!("{:x}", now() * 1000 + 7), time: now(), kind: "delete".into(), name: last_segment(&p),
        item_kind: item_kind.into(), from: p, to: None, files: 1, bytes, zip_bytes: 0, verified: true, restored: None,
    };
    let mut s = load_state();
    s.actions.push(a.clone());
    save_state(&s)?;
    Ok(a)
}

/// Undo an archive (unzip back in place, verify, zip to the Recycle Bin) or a delete
/// (restore from the Recycle Bin).
pub fn restore(id: &str, prog: &Progress) -> Result<String, String> {
    let mut s = load_state();
    let a = s.actions.iter().find(|a| a.id == id).cloned().ok_or("That action isn't in the log")?;
    if a.restored.is_some() {
        return Err("Already restored".into());
    }
    let target = PathBuf::from(&a.from);
    if target.exists() {
        return Err(format!("Something is already at {}, so nothing was overwritten", a.from));
    }
    if a.kind == "archive" {
        let zp = a.to.clone().ok_or("No zip recorded")?;
        let mut za = zip::ZipArchive::new(File::open(&zp).map_err(|e| format!("Can't open the zip: {e}"))?).map_err(|e| e.to_string())?;
        let single_file = za.len() == 1 && !za.by_index(0).map(|f| f.is_dir()).unwrap_or(true) && a.item_kind == "download" && a.files == 1;
        (prog.report)("unzipping", 0, a.bytes);
        if single_file && !a.from.is_empty() && za.by_index(0).map(|f| f.name() == a.name).unwrap_or(false) {
            let mut zf = za.by_index(0).map_err(|e| e.to_string())?;
            let mut out = File::create(&target).map_err(|e| e.to_string())?;
            io::copy(&mut zf, &mut out).map_err(|e| e.to_string())?;
        } else {
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            za.extract(&target).map_err(|e| format!("Unzip failed: {e}"))?;
        }
        // verify the count came back
        let back = if target.is_file() { 1 } else { walkdir::WalkDir::new(&target).into_iter().flatten().filter(|e| e.file_type().is_file()).count() as u64 };
        if back != a.files {
            return Err(format!("Restored {back} of {} files; the zip was kept at {zp}", a.files));
        }
        (prog.report)("moving the zip to the Recycle Bin", a.bytes, a.bytes);
        let _ = trash::delete(&zp);
    } else {
        let items = trash::os_limited::list().map_err(|e| e.to_string())?;
        let hit: Vec<_> = items
            .into_iter()
            .filter(|i| key(&i.original_parent.join(&i.name).to_string_lossy()) == key(&a.from))
            .collect();
        if hit.is_empty() {
            return Err("It's no longer in the Recycle Bin".into());
        }
        let newest = hit.into_iter().max_by_key(|i| i.time_deleted).unwrap();
        trash::os_limited::restore_all([newest]).map_err(|e| e.to_string())?;
    }
    if let Some(x) = s.actions.iter_mut().find(|x| x.id == id) {
        x.restored = Some(now());
    }
    save_state(&s)?;
    Ok(a.from)
}

/// `--cleanup-selftest <scratch archive dir>`: archive + restore and delete + restore a
/// throwaway folder it creates itself, and check every byte comes back. Never touches
/// anything it didn't create.
pub fn selftest(archive_to: &str) -> String {
    use std::fmt::Write as _;
    let mut log = String::new();
    std::env::set_var("CARTOGRAPHER_ARCHIVE_DIR", archive_to);
    std::env::set_var("CARTOGRAPHER_STATE_FILE", PathBuf::from(archive_to).join("selftest-cleanup.json"));
    let docs = format!(r"{}\Documents", norm(&std::env::var("USERPROFILE").unwrap_or_default()));
    let src = PathBuf::from(format!(r"{docs}\_cartographer_selftest_{}", now()));
    let mut seed: u64 = 0x9E3779B97F4A7C15;
    let mut rnd = |n: usize| -> Vec<u8> {
        (0..n).map(|_| { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; (seed & 0xff) as u8 }).collect()
    };
    let files: Vec<(String, Vec<u8>)> = vec![
        ("a.txt".into(), b"hello cartographer".to_vec()),
        (r"sub\b.bin".into(), rnd(3 * 1024 * 1024 + 17)),
        (r"sub\deeper\c.bin".into(), rnd(70_000)),
        ("zero.dat".into(), vec![]),
    ];
    let quiet = |_: &str, _: u64, _: u64| {};
    let prog = Progress { report: &quiet };
    let check = |label: &str, ok: bool, log: &mut String| { let _ = writeln!(log, "{} {label}", if ok { "PASS" } else { "FAIL" }); };

    std::fs::create_dir_all(src.join(r"sub\deeper")).ok();
    std::fs::create_dir_all(src.join("empty")).ok();
    for (rel, data) in &files { std::fs::write(src.join(rel), data).ok(); }
    let src_s = src.to_string_lossy().to_string();

    match archive(&src_s, "project", &prog) {
        Ok(a) => {
            check("archive: originals gone", !src.exists(), &mut log);
            check(&format!("archive: zip exists ({} files, {} bytes -> {} zip)", a.files, a.bytes, a.zip_bytes), a.to.as_ref().map(|z| Path::new(z).exists()).unwrap_or(false), &mut log);
            check("archive: verified", a.verified && a.files == files.len() as u64, &mut log);
            match restore(&a.id, &prog) {
                Ok(_) => {
                    let same = files.iter().all(|(rel, data)| std::fs::read(src.join(rel)).map(|b| &b == data).unwrap_or(false));
                    check("restore: every byte matches", same, &mut log);
                    check("restore: empty folder came back", src.join("empty").is_dir(), &mut log);
                    check("restore: zip moved to Recycle Bin", !Path::new(a.to.as_ref().unwrap()).exists(), &mut log);
                }
                Err(e) => { let _ = writeln!(log, "FAIL restore: {e}"); }
            }
        }
        Err(e) => { let _ = writeln!(log, "FAIL archive: {e}"); }
    }
    // refuse dangerous paths
    check(r"safety: refuses C:\Windows", allowed(r"C:\Windows\System32").is_err(), &mut log);
    check("safety: refuses a drive root", allowed("E:\\").is_err(), &mut log);
    check("safety: refuses AppData", allowed(&format!(r"{}\AppData\Local", norm(&std::env::var("USERPROFILE").unwrap_or_default()))).is_err(), &mut log);
    check("safety: refuses Documents itself", allowed(&docs).is_err(), &mut log);

    // delete + restore a single file
    let f = src.join("a.txt");
    if f.exists() {
        match delete(&f.to_string_lossy(), "download") {
            Ok(a) => {
                check("delete: file gone", !f.exists(), &mut log);
                match restore(&a.id, &prog) {
                    Ok(_) => check("delete-restore: file back from Recycle Bin", std::fs::read(&f).map(|b| b == b"hello cartographer").unwrap_or(false), &mut log),
                    Err(e) => { let _ = writeln!(log, "FAIL delete-restore: {e}"); }
                }
            }
            Err(e) => { let _ = writeln!(log, "FAIL delete: {e}"); }
        }
    }
    let _ = std::fs::remove_dir_all(&src); // our own test folder
    log
}
