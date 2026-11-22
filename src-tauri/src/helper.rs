//! The elevated "fast scan" helper.
//!
//! Reading the MFT needs admin, but the app itself never runs elevated. Instead, a one-time
//! UAC prompt (`--install-helper`) copies this exe to Program Files (admin-only, so nothing
//! else can swap it out) and registers a scheduled task that runs it with highest privileges.
//! The app then starts that task on demand (`schtasks /Run`, no prompt). The two talk through
//! two small files in the app data folder: a request (which drive) and a status (progress / result).

use crate::{db, mft, scan};
use serde::{Deserialize, Serialize};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const TASK: &str = r"\Cartographer\Fast Scan";
const NO_WINDOW: u32 = 0x0800_0000;

pub fn data_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("app.cartographer.desktop")
}
fn request_path() -> PathBuf { data_dir().join("fast-scan-request.json") }
pub fn status_path() -> PathBuf { data_dir().join("fast-scan-status.json") }

fn helper_exe() -> PathBuf {
    let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
    PathBuf::from(pf).join("Cartographer").join("cartographer-helper.exe")
}

#[derive(Serialize, Deserialize)]
pub struct Request {
    pub root: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Status {
    pub state: String, // "running" | "done" | "error"
    pub root: String,
    pub phase: String, // "reading index" | "building map" | "saving"
    pub records: u64,
    pub total_records: u64,
    pub files: u64,
    pub bytes: u64,
    pub seconds: f64,
    pub scan_id: Option<i64>,
    pub error: Option<String>,
    pub at: u64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn valid_root(r: &str) -> bool {
    let b = r.as_bytes();
    b.len() == 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

fn write_status(s: &Status) {
    let p = status_path();
    let tmp = p.with_extension("tmp");
    if let Ok(j) = serde_json::to_vec(s) {
        if std::fs::write(&tmp, j).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

pub fn read_status() -> Option<Status> {
    serde_json::from_slice(&std::fs::read(status_path()).ok()?).ok()
}

fn schtasks(args: &[&str]) -> bool {
    Command::new("schtasks")
        .args(args)
        .creation_flags(NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn installed() -> bool {
    helper_exe().exists() && schtasks(&["/Query", "/TN", TASK])
}

/// Runs `this exe <arg>` elevated (UAC prompt) and waits. Returns false if the user said no.
pub fn run_elevated(arg: &str) -> Result<bool, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy().replace('\'', "''");
    let ps = format!(
        "try {{ $p = Start-Process -FilePath '{exe}' -ArgumentList '{arg}' -Verb RunAs -Wait -PassThru; exit $p.ExitCode }} catch {{ exit 1223 }}"
    );
    let st = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .creation_flags(NO_WINDOW)
        .status()
        .map_err(|e| e.to_string())?;
    Ok(st.code() == Some(0))
}

/// `--install-helper` (already elevated).
pub fn install() -> i32 {
    let dst = helper_exe();
    let ok = (|| -> Result<(), String> {
        std::fs::create_dir_all(dst.parent().unwrap()).map_err(|e| e.to_string())?;
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        std::fs::copy(me, &dst).map_err(|e| e.to_string())?;
        let tr = format!("\"{}\" --helper", dst.display());
        if !schtasks(&["/Create", "/F", "/TN", TASK, "/TR", &tr, "/SC", "ONCE", "/ST", "00:00", "/RL", "HIGHEST"]) {
            return Err("schtasks /Create failed".into());
        }
        Ok(())
    })();
    if ok.is_ok() { 0 } else { 1 }
}

/// `--uninstall-helper` (already elevated).
pub fn uninstall() -> i32 {
    schtasks(&["/Delete", "/F", "/TN", TASK]);
    let _ = std::fs::remove_file(helper_exe());
    if let Some(d) = helper_exe().parent() {
        let _ = std::fs::remove_dir(d);
    }
    0
}

/// Ask the helper to scan `root`. Returns false if the task couldn't be started.
pub fn request(root: &str) -> bool {
    if !valid_root(root) {
        return false;
    }
    let _ = std::fs::create_dir_all(data_dir());
    let _ = std::fs::remove_file(status_path());
    let body = serde_json::to_vec(&Request { root: root.to_string() }).unwrap_or_default();
    if std::fs::write(request_path(), body).is_err() {
        return false;
    }
    schtasks(&["/Run", "/TN", TASK])
}

/// `--helper`: what the scheduled task runs (elevated). Scans the requested drive, saves to the index.
pub fn run() -> i32 {
    let req: Option<Request> = std::fs::read(request_path()).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let _ = std::fs::remove_file(request_path());
    let Some(req) = req else { return 2 };
    if !valid_root(&req.root) {
        return 2;
    }
    let root = req.root.to_uppercase();
    let prog = Arc::new(scan::Progress::default());
    let stop = Arc::new(AtomicBool::new(false));
    let t0 = Instant::now();
    let phase = Arc::new(std::sync::Mutex::new("reading index".to_string()));

    let ticker = {
        let (prog, stop, root, phase) = (prog.clone(), stop.clone(), root.clone(), phase.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                write_status(&Status {
                    state: "running".into(),
                    root: root.clone(),
                    phase: phase.lock().map(|p| p.clone()).unwrap_or_default(),
                    records: prog.records.load(Ordering::Relaxed),
                    total_records: prog.total_records.load(Ordering::Relaxed),
                    files: prog.files.load(Ordering::Relaxed),
                    bytes: prog.bytes.load(Ordering::Relaxed),
                    seconds: t0.elapsed().as_secs_f64(),
                    at: now(),
                    ..Default::default()
                });
                std::thread::sleep(Duration::from_millis(150));
            }
        })
    };

    let started = now() as i64;
    let result = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn({
            let (prog, phase, root) = (prog.clone(), phase.clone(), root.clone());
            move || -> Result<i64, String> {
                let tree = mft::scan(&root, &prog)?;
                *phase.lock().unwrap() = "saving".into();
                let ms = t0.elapsed().as_millis() as i64;
                let _ = std::fs::create_dir_all(data_dir());
                let mut conn = db::open(&data_dir().join("index.db")).map_err(|e| e.to_string())?;
                db::save_scan(&mut conn, &root, started, ms, 0, "fast", &tree).map_err(|e| e.to_string())
            }
        })
        .map_err(|e| e.to_string())
        .and_then(|h| h.join().map_err(|_| "scan crashed".to_string())?);

    stop.store(true, Ordering::Relaxed);
    let _ = ticker.join();
    let mut s = Status {
        root,
        records: prog.records.load(Ordering::Relaxed),
        total_records: prog.total_records.load(Ordering::Relaxed),
        files: prog.files.load(Ordering::Relaxed),
        bytes: prog.bytes.load(Ordering::Relaxed),
        seconds: t0.elapsed().as_secs_f64(),
        at: now(),
        ..Default::default()
    };
    match result {
        Ok(id) => { s.state = "done".into(); s.scan_id = Some(id); write_status(&s); 0 }
        Err(e) => { s.state = "error".into(); s.error = Some(e); write_status(&s); 1 }
    }
}
