#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ask;
mod ask_tools;
mod db;
mod backups;
mod cleanup;
mod drives;
mod graph;
mod health;
mod helper;
mod history;
mod idle;
mod mft;
mod projects;
mod scan;
mod screensaver;
mod watch;

use rusqlite::Connection;
use serde::Serialize;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder};

struct AppState {
    db_path: PathBuf,
    conn: Mutex<Connection>,
    scanning: Arc<AtomicBool>,
    showing: Arc<AtomicBool>,
    convo: Mutex<ask::Conversation>,
}

#[derive(Serialize, Clone)]
struct ScanProgress {
    root: String,
    phase: String,
    files: u64,
    dirs: u64,
    bytes: u64,
    records: u64,
    total_records: u64,
    seconds: f64,
}

#[derive(Serialize, Clone)]
struct ScanDone {
    root: String,
    scan_id: i64,
}

pub fn fmt_gb(b: i64) -> String {
    format!("{:.1} GB", b as f64 / 1_073_741_824.0)
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[tauri::command(async)]
fn drives() -> Vec<drives::Drive> {
    drives::list()
}

#[tauri::command(async)]
fn scans(state: State<AppState>) -> Result<Vec<db::ScanInfo>, String> {
    let c = state.conn.lock().map_err(err)?;
    db::scans(&c).map_err(err)
}

#[tauri::command(async)]
fn view(state: State<AppState>, scan_id: i64, dir_id: Option<i64>) -> Result<Option<db::View>, String> {
    let c = state.conn.lock().map_err(err)?;
    let root: String = c
        .query_row("SELECT root FROM scans WHERE id = ?1", [scan_id], |r| r.get(0))
        .map_err(err)?;
    db::view(&c, scan_id, dir_id, drives::used(&root)).map_err(err)
}

#[tauri::command(async)]
fn is_scanning(state: State<AppState>) -> bool {
    state.scanning.load(Ordering::SeqCst)
}

#[tauri::command(async)]
fn start_scan(app: AppHandle, root: String) -> Result<(), String> {
    begin_scan(&app, root)
}

/// Starts a background scan of `root` (used by the Scan button and by live updates).
fn begin_scan(app: &AppHandle, root: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.scanning.swap(true, Ordering::SeqCst) {
        return Err("A scan is already running".into());
    }
    let flag = state.scanning.clone();
    let db_path = state.db_path.clone();
    let app = app.clone();
    std::thread::Builder::new()
        .name("scan".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let _ = app.emit("scan-started", root.clone());
            let result = match fast_scan(&app, &root) {
                Some(r) => r,
                None => run_scan(&app, &db_path, &root),
            };
            flag.store(false, Ordering::SeqCst);
            match result {
                Ok(scan_id) => {
                    if let Ok(mut c) = db::open(&db_path) {
                        let _ = history::record(&mut c, scan_id);
                    }
                    let _ = app.emit("scan-done", ScanDone { root, scan_id });
                }
                Err(e) => { let _ = app.emit("scan-error", e); }
            }
        })
        .map_err(err)?;
    Ok(())
}

/// Use the elevated helper when it's installed and the drive is NTFS.
/// Returns None to fall back to the normal folder walk.
fn fast_scan(app: &AppHandle, root: &str) -> Option<Result<i64, String>> {
    let letter = root.chars().next()?.to_ascii_uppercase().to_string();
    let ntfs = drives::list().into_iter().any(|d| d.letter == letter && d.fs == "NTFS");
    if !ntfs || !helper::installed() || !helper::request(root) {
        return None;
    }
    let t0 = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(150));
        let s = helper::read_status();
        match s {
            Some(s) if s.state == "done" => return Some(s.scan_id.ok_or_else(|| "helper gave no result".to_string())),
            Some(s) if s.state == "error" => {
                let e = s.error.unwrap_or_default();
                return if e.contains("not NTFS") { None } else { Some(Err(e)) };
            }
            Some(s) => {
                let _ = app.emit("scan-progress", ScanProgress {
                    root: root.to_string(), phase: s.phase, files: s.files, dirs: 0, bytes: s.bytes,
                    records: s.records, total_records: s.total_records, seconds: s.seconds,
                });
            }
            None if t0.elapsed() > Duration::from_secs(20) => return None, // helper never started
            None => {}
        }
        if t0.elapsed() > Duration::from_secs(900) {
            return Some(Err("Fast scan timed out".into()));
        }
    }
}

fn run_scan(app: &AppHandle, db_path: &PathBuf, root: &str) -> Result<i64, String> {
    let prog = Arc::new(scan::Progress::default());
    let done = Arc::new(AtomicBool::new(false));
    let t0 = Instant::now();
    let started = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);

    // progress ticker
    {
        let (prog, done, app, root) = (prog.clone(), done.clone(), app.clone(), root.to_string());
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                let _ = app.emit("scan-progress", ScanProgress {
                    root: root.clone(),
                    phase: "walking folders".into(),
                    records: 0,
                    total_records: 0,
                    files: prog.files.load(Ordering::Relaxed),
                    dirs: prog.dirs.load(Ordering::Relaxed),
                    bytes: prog.bytes.load(Ordering::Relaxed),
                    seconds: t0.elapsed().as_secs_f64(),
                });
                std::thread::sleep(Duration::from_millis(150));
            }
        });
    }

    let name = root.trim_end_matches('\\').to_string();
    let tree = scan::scan(std::path::Path::new(root), name, &prog);
    let scan_ms = t0.elapsed().as_millis() as i64;
    let mut conn = db::open(db_path).map_err(err)?;
    let id = db::save_scan(&mut conn, root, started, scan_ms, prog.denied.load(Ordering::Relaxed), "walk", &tree).map_err(err);
    done.store(true, Ordering::Relaxed);
    id
}

#[tauri::command(async)]
fn graph(state: State<AppState>) -> Result<graph::GraphData, String> {
    let c = state.conn.lock().map_err(err)?;
    graph::build(&c).map_err(err)
}

#[tauri::command(async)]
fn backup_report(state: State<AppState>) -> Result<backups::Report, String> {
    let c = state.conn.lock().map_err(err)?;
    backups::report(&c).map_err(err)
}

#[tauri::command(async)]
fn cleanup_report(state: State<AppState>) -> Result<cleanup::Report, String> {
    let c = state.conn.lock().map_err(err)?;
    cleanup::report(&c).map_err(err)
}

#[tauri::command(async)]
fn cleanup_hide(key: String, days: Option<i64>) -> Result<(), String> {
    match days {
        Some(d) => cleanup::snooze(&key, d),
        None => cleanup::never(&key),
    }
}

#[tauri::command(async)]
fn cleanup_unhide_all() -> Result<(), String> {
    cleanup::unhide_all()
}

#[derive(Serialize, Clone)]
struct CleanupProgress {
    job: String,
    phase: String,
    done: u64,
    total: u64,
}

#[derive(Serialize, Clone)]
struct CleanupDone {
    job: String,
    ok: bool,
    message: String,
}

/// One archive / delete / restore, running or waiting in the queue.
#[derive(Serialize, Clone)]
struct CleanupJob {
    job: String,
    label: String,
    op: String,
    path: Option<String>,
    kind: Option<String>,
    id: Option<String>,
    phase: String,
    done: u64,
    total: u64,
}

/// What Cleanup is doing: the job that's running, the ones queued after it (run one at a time,
/// in order), and results you haven't dismissed yet. Kept here, not in the Cleanup tab, so it
/// survives switching tabs or closing and reopening the window.
#[derive(Serialize, Clone, Default)]
struct CleanupJobs {
    running: Option<CleanupJob>,
    queue: Vec<CleanupJob>,
    results: Vec<CleanupDone>,
}

static CLEANUP_JOBS: Mutex<CleanupJobs> = Mutex::new(CleanupJobs { running: None, queue: Vec::new(), results: Vec::new() });
static CLEANUP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn cleanup_snapshot() -> CleanupJobs {
    CLEANUP_JOBS.lock().map(|j| j.clone()).unwrap_or_default()
}

/// Sends an event to the window (or, in the queue self-test, nowhere).
type Emit = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

fn emitter(app: AppHandle) -> Emit {
    Arc::new(move |name: &str, v: serde_json::Value| {
        let _ = app.emit(name, v);
    })
}

/// Tells every open view (Cleanup tab, tab-bar spinner) what the queue looks like now.
fn cleanup_changed(emit: &Emit) {
    emit("cleanup-jobs", serde_json::to_value(cleanup_snapshot()).unwrap_or_default());
}

#[tauri::command]
fn cleanup_jobs() -> CleanupJobs {
    cleanup_snapshot()
}

/// You dismissed a result; don't show it again.
#[tauri::command]
fn cleanup_ack(app: AppHandle, job: String) {
    if let Ok(mut j) = CLEANUP_JOBS.lock() {
        j.results.retain(|r| r.job != job);
    }
    cleanup_changed(&emitter(app));
}

/// Takes a job out of the queue before it starts (the running one can't be stopped halfway).
#[tauri::command]
fn cleanup_unqueue(app: AppHandle, job: String) {
    if let Ok(mut j) = CLEANUP_JOBS.lock() {
        j.queue.retain(|q| q.job != job);
    }
    cleanup_changed(&emitter(app));
}

/// Archive / delete / restore. Starts now if nothing is running, otherwise joins the queue and
/// starts by itself when the ones before it finish. Progress and results arrive as events.
/// Jobs never overlap: two archives touching the same files would fight over them.
#[tauri::command(async)]
fn cleanup_run(app: AppHandle, op: String, path: Option<String>, kind: Option<String>, id: Option<String>, label: Option<String>) -> Result<String, String> {
    cleanup_enqueue(emitter(app), op, path, kind, id, label)
}

fn cleanup_enqueue(emit: Emit, op: String, path: Option<String>, kind: Option<String>, id: Option<String>, label: Option<String>) -> Result<String, String> {
    let n = CLEANUP_SEQ.fetch_add(1, Ordering::SeqCst);
    let job = format!("{}-{}-{n}", op, cleanup::now());
    let task = CleanupJob {
        job: job.clone(), label: label.unwrap_or_else(|| op.clone()), op, path, kind, id,
        phase: "starting".into(), done: 0, total: 0,
    };
    let start = {
        let mut jobs = CLEANUP_JOBS.lock().map_err(err)?;
        let same = |o: &CleanupJob| (task.path.is_some() && o.path == task.path) || (task.id.is_some() && o.id == task.id);
        if jobs.running.iter().chain(jobs.queue.iter()).any(same) {
            return Err(format!("\"{}\" is already running or in the queue.", task.label));
        }
        if jobs.running.is_none() {
            jobs.running = Some(task.clone());
            true
        } else {
            jobs.queue.push(task.clone());
            false
        }
    };
    cleanup_changed(&emit);
    if start {
        std::thread::spawn(move || cleanup_worker(emit, task));
    }
    Ok(job)
}

/// Runs `first`, then each queued job in turn until the queue is empty. Keeps the PC from going
/// to sleep while it works (Windows' own request, released when the queue is done).
fn cleanup_worker(emit: Emit, first: CleanupJob) {
    use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};
    unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
    let mut task = Some(first);
    while let Some(t) = task {
        let j = t.job.clone();
        let last = std::sync::Mutex::new(std::time::Instant::now() - Duration::from_secs(1));
        let report = |phase: &str, done: u64, total: u64| {
            let mut l = last.lock().unwrap();
            if l.elapsed() >= Duration::from_millis(120) || done == total {
                *l = std::time::Instant::now();
                if let Ok(mut jobs) = CLEANUP_JOBS.lock() {
                    if let Some(r) = jobs.running.as_mut() {
                        r.phase = phase.into();
                        r.done = done;
                        r.total = total;
                    }
                }
                emit("cleanup-progress", serde_json::to_value(CleanupProgress { job: j.clone(), phase: phase.into(), done, total }).unwrap_or_default());
            }
        };
        let prog = cleanup::Progress { report: &report };
        let (path, kind, id) = (t.path.as_deref().unwrap_or(""), t.kind.as_deref().unwrap_or("other"), t.id.as_deref().unwrap_or(""));
        // catch_unwind: a crash in one job must not stall the queue or leave Cleanup locked
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match t.op.as_str() {
            "archive" => cleanup::archive(path, kind, &prog)
                .map(|a| format!("Archived {} ({} files) to {}", a.name, a.files, a.to.unwrap_or_default())),
            "delete" => cleanup::delete(path, kind).map(|a| format!("{} is in the Recycle Bin", a.name)),
            "restore" => cleanup::restore(id, &prog).map(|p| format!("Restored to {p}")),
            _ => Err("unknown operation".into()),
        }))
        .unwrap_or_else(|_| Err("Something went wrong partway through. Check the Archive folder and the Recycle Bin.".into()));
        let (ok, message) = match res { Ok(m) => (true, m), Err(e) => (false, format!("{}: {e}", t.label)) };
        let done = CleanupDone { job: j, ok, message };
        task = match CLEANUP_JOBS.lock() {
            Ok(mut jobs) => {
                jobs.results.push(done.clone());
                let extra = jobs.results.len().saturating_sub(30);
                jobs.results.drain(..extra);
                let next = if jobs.queue.is_empty() { None } else { Some(jobs.queue.remove(0)) };
                jobs.running = next.clone();
                next
            }
            Err(_) => None,
        };
        emit("cleanup-done", serde_json::to_value(&done).unwrap_or_default());
        cleanup_changed(&emit);
    }
    unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
}

/// `--queue-selftest <scratch archive dir> <out>`: queues archives of three throwaway folders it
/// makes in Documents, removes one from the queue, checks a duplicate is refused and they run
/// one at a time in order, then queues restores to put everything back. Touches nothing else.
fn queue_selftest(archive_to: &str) -> String {
    use std::fmt::Write as _;
    use std::path::Path;
    std::env::set_var("CARTOGRAPHER_ARCHIVE_DIR", archive_to);
    std::env::set_var("CARTOGRAPHER_STATE_FILE", PathBuf::from(archive_to).join("selftest-cleanup.json"));
    let mut log = String::new();
    let check = |log: &mut String, label: &str, ok: bool| { let _ = writeln!(log, "{} {label}", if ok { "PASS" } else { "FAIL" }); };
    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    let emit: Emit = Arc::new(move |name: &str, v: serde_json::Value| {
        if name == "cleanup-done" {
            ev.lock().unwrap().push(format!("{}|{}|{}", v["job"].as_str().unwrap_or(""), v["ok"], v["message"].as_str().unwrap_or("")));
        }
    });
    let docs = format!(r"{}\Documents", std::env::var("USERPROFILE").unwrap_or_default());
    let t = cleanup::now();
    let mk = |name: &str, mb: usize| {
        let d = PathBuf::from(format!(r"{docs}\_cartographer_queuetest_{t}_{name}"));
        std::fs::create_dir_all(d.join("sub")).ok();
        let mut x: u64 = 0x2545F4914F6CDD1D ^ mb as u64;
        let data: Vec<u8> = (0..mb * 1024 * 1024).map(|_| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x as u8 }).collect();
        std::fs::write(d.join("sub").join("data.bin"), &data).ok();
        std::fs::write(d.join("readme.txt"), name).ok();
        d.to_string_lossy().to_string()
    };
    let (a, b, c) = (mk("A", 150), mk("B", 5), mk("C", 5));
    let q = |p: &str, l: &str| cleanup_enqueue(emit.clone(), "archive".into(), Some(p.into()), Some("project".into()), None, Some(l.into()));
    let ja = q(&a, "Archive A");
    let jb = q(&b, "Archive B");
    let dup = q(&a, "Archive A again");
    let jc = q(&c, "Archive C");
    check(&mut log, "A starts, B and C queue", ja.is_ok() && jb.is_ok() && jc.is_ok());
    check(&mut log, &format!("same folder twice is refused ({})", dup.as_ref().err().cloned().unwrap_or_default()), dup.is_err());
    let snap = cleanup_snapshot();
    let ok = snap.running.as_ref().map_or(false, |r| r.label == "Archive A") && snap.queue.len() == 2;
    check(&mut log, &format!("A running, 2 waiting (running={} queue={})", snap.running.as_ref().map_or("-", |r| r.label.as_str()), snap.queue.len()), ok);
    if let Ok(jc) = &jc {
        if let Ok(mut j) = CLEANUP_JOBS.lock() {
            j.queue.retain(|x| &x.job != jc);
        }
    }
    let wait = || {
        let t0 = Instant::now();
        while cleanup_snapshot().running.is_some() && t0.elapsed() < Duration::from_secs(300) {
            std::thread::sleep(Duration::from_millis(200));
        }
    };
    wait();
    let evs: Vec<String> = events.lock().unwrap().clone();
    for e in &evs {
        let _ = writeln!(log, "     done: {e}");
    }
    let in_order = evs.len() == 2
        && evs[0].starts_with(ja.as_deref().unwrap_or("?"))
        && evs[1].starts_with(jb.as_deref().unwrap_or("?"))
        && evs.iter().all(|e| e.contains("|true|"));
    check(&mut log, "both ran, A then B, both ok", in_order);
    check(&mut log, "A and B moved out", !Path::new(&a).exists() && !Path::new(&b).exists());
    check(&mut log, "C (removed from queue) untouched", Path::new(&c).join("readme.txt").exists());
    let snap = cleanup_snapshot();
    check(&mut log, "queue empty, 2 results kept", snap.queue.is_empty() && snap.results.len() == 2);

    // put it back: queue both restores at once
    let st = cleanup::load_state();
    for act in st.actions.iter().filter(|x| x.restored.is_none() && (x.from == crate::graph::norm(&a) || x.from == crate::graph::norm(&b))) {
        let _ = cleanup_enqueue(emit.clone(), "restore".into(), None, None, Some(act.id.clone()), Some(format!("Restore {}", act.name)));
    }
    wait();
    check(&mut log, "queued restores brought A and B back", Path::new(&a).join("sub").join("data.bin").exists() && Path::new(&b).join("readme.txt").exists());
    for d in [&a, &b, &c] {
        let _ = std::fs::remove_dir_all(d);
    }
    log
}

#[tauri::command(async)]
fn open_uninstall() -> Result<(), String> {
    std::process::Command::new("explorer").arg("ms-settings:appsfeatures").spawn().map(|_| ()).map_err(err)
}

#[tauri::command(async)]
fn get_settings() -> idle::Settings {
    idle::load()
}

#[tauri::command(async)]
fn set_settings(settings: idle::Settings) -> Result<(), String> {
    idle::save(&settings)
}

/// Why show mode wouldn't start right now, plus whether OBS is open.
#[tauri::command(async)]
fn show_status() -> serde_json::Value {
    serde_json::json!({ "blocked": idle::blocked_reason(), "obs": idle::obs_running(), "idle_secs": idle::idle_secs() })
}

#[tauri::command(async)]
fn show_preview(app: AppHandle) {
    idle::open_show(&app);
}

#[tauri::command(async)]
fn show_exit(app: AppHandle) {
    idle::close_show(&app);
}

#[tauri::command(async)]
fn health_report(state: State<AppState>) -> Result<health::Report, String> {
    let c = state.conn.lock().map_err(err)?;
    health::report(&c).map_err(err)
}

#[tauri::command(async)]
fn timeline(state: State<AppState>, days: i64) -> Result<history::Timeline, String> {
    let mut c = state.conn.lock().map_err(err)?;
    history::report(&mut c, days).map_err(err)
}

#[tauri::command(async)]
fn projects_report(state: State<AppState>) -> Result<projects::Report, String> {
    let c = state.conn.lock().map_err(err)?;
    projects::report(&c).map_err(err)
}

#[tauri::command(async)]
fn open_in_editor(path: String) -> Result<(), String> {
    projects::open_in_editor(&path)
}

#[tauri::command(async)]
fn open_terminal(path: String) -> Result<(), String> {
    projects::open_terminal(&path)
}

/// Opens a web page (a project's GitHub link) in the default browser.
#[tauri::command(async)]
fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("Only https links".into());
    }
    std::process::Command::new("explorer").raw_arg(format!("\"{}\"", url)).spawn().map(|_| ()).map_err(err)
}

#[tauri::command(async)]
fn screensaver_status() -> bool {
    screensaver::installed()
}

/// Opens Windows' Screen Saver Settings with Cartographer selected (you press OK there).
#[tauri::command(async)]
fn screensaver_install() -> Result<String, String> {
    screensaver::install()
}

#[tauri::command(async)]
fn ask_status() -> ask::Status {
    ask::status()
}

/// Stores (or with an empty string, removes) the Anthropic API key, encrypted for this Windows user.
#[tauri::command(async)]
fn ask_set_key(key: String) -> Result<ask::Status, String> {
    ask::save_key(&key)?;
    Ok(ask::status())
}

#[tauri::command(async)]
fn ask_new(state: State<AppState>) {
    if let Ok(mut c) = state.convo.lock() {
        *c = ask::Conversation::default();
    }
}

#[tauri::command(async)]
fn ask_question(app: AppHandle, state: State<AppState>, question: String) -> Result<ask::Answer, String> {
    let s = idle::load();
    if !s.ask_ok {
        return Err("Turn on Ask Claude first.".into());
    }
    let mut convo = state.convo.try_lock().map_err(|_| "Claude is still answering the last question.".to_string())?;
    let emit = |st: &ask::Step| { let _ = app.emit("ask-step", st.clone()); };
    ask::ask(&state.db_path, &mut convo, &question, &s.ask_backend, &emit)
}

/// The map's layout (forces, radiate/pull/push points, pinned and last positions).
#[tauri::command(async)]
fn load_layout() -> Option<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(helper::data_dir().join("layout.json")).ok()?).ok()
}

#[tauri::command(async)]
fn save_layout(layout: serde_json::Value) -> Result<(), String> {
    let p = helper::data_dir().join("layout.json");
    std::fs::create_dir_all(helper::data_dir()).map_err(err)?;
    std::fs::write(&p, serde_json::to_vec_pretty(&layout).map_err(err)?).map_err(err)
}

#[tauri::command(async)]
fn reset_layout() -> Result<(), String> {
    let p = helper::data_dir().join("layout.json");
    if p.exists() {
        std::fs::remove_file(p).map_err(err)?;
    }
    Ok(())
}

#[tauri::command(async)]
fn helper_status() -> bool {
    helper::installed()
}

/// Shows one UAC prompt. Returns whether fast scan is now on.
#[tauri::command(async)]
fn set_fast_scan(on: bool) -> Result<bool, String> {
    helper::run_elevated(if on { "--install-helper" } else { "--uninstall-helper" })?;
    Ok(helper::installed())
}

#[tauri::command(async)]
fn reveal(path: String) -> Result<(), String> {
    std::process::Command::new("explorer")
        .raw_arg(format!("/select,\"{}\"", path))
        .spawn()
        .map(|_| ())
        .map_err(err)
}

#[tauri::command(async)]
fn open_path(path: String) -> Result<(), String> {
    std::process::Command::new("explorer").raw_arg(format!("\"{}\"", path)).spawn().map(|_| ()).map_err(err)
}

/// Show the main window, creating it if it was closed (closing frees the UI's memory;
/// the tray app keeps running).
fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("Cartographer")
        .inner_size(1360.0, 860.0)
        .min_inner_size(960.0, 600.0)
        .background_color(tauri::window::Color(11, 15, 23, 255))
        .build();
}

/// `cartographer.exe --bench C:\ <db> <report>`: scan without the UI and write timings (for testing).
fn bench(args: &[String]) {
    let root = args.get(2).cloned().unwrap_or_else(|| "C:\\".into());
    let db_path = PathBuf::from(args.get(3).cloned().unwrap_or_else(|| "bench.db".into()));
    let report = args.get(4).cloned().unwrap_or_else(|| "bench.txt".into());
    let fast = args.get(1).map(|s| s == "--bench-fast").unwrap_or(false);
    let prog = scan::Progress::default();
    let t0 = Instant::now();
    let tree = if fast {
        match mft::scan(&root, &prog) {
            Ok(t) => t,
            Err(e) => {
                let _ = std::fs::write(&report, format!("fast scan failed: {e}\n"));
                return;
            }
        }
    } else {
        scan::scan(std::path::Path::new(&root), root.trim_end_matches('\\').into(), &prog)
    };
    let scan_s = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    let mut conn = db::open(&db_path).expect("db");
    let id = db::save_scan(&mut conn, &root, 0, (scan_s * 1000.0) as i64, prog.denied.load(Ordering::Relaxed), if fast { "fast" } else { "walk" }, &tree).expect("save");
    let save_s = t1.elapsed().as_secs_f64();
    let t2 = Instant::now();
    let v = db::view(&conn, id, None, drives::used(&root)).expect("view").expect("root");
    let view_s = t2.elapsed().as_secs_f64();
    let mut out = format!(
        "root {root}\nscan {scan_s:.2}s  save {save_s:.2}s  view {view_s:.3}s\nfiles {}  dirs {}  bytes {:.1} GB  denied dirs {}\nused on disk {:.1} GB\n\ntop of view ({} items):\n",
        tree.files, tree.dirs, tree.size as f64 / 1e9, prog.denied.load(Ordering::Relaxed),
        drives::used(&root).unwrap_or(0) as f64 / 1e9,
        v.node.children.as_ref().map(|c| c.len()).unwrap_or(0)
    );
    for k in v.node.children.unwrap_or_default().iter().take(12) {
        out += &format!("  {:>8.1} GB  {:<6} {}\n", k.size as f64 / 1e9, k.kind, k.name);
    }
    let _ = std::fs::write(report, out);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Started by Windows as Cartographer.scr
    match screensaver::mode(args.get(1).map(|s| s.as_str())) {
        Some(screensaver::Mode::Preview) => return, // no tiny preview
        Some(screensaver::Mode::Show) => {
            if screensaver::hand_off() {
                return; // the running Cartographer showed it
            }
            screensaver::ONLY.store(true, Ordering::SeqCst); // nothing running: be the screensaver ourselves
        }
        Some(screensaver::Mode::Configure) | None => {}
    }
    match args.get(1).map(|s| s.as_str()) {
        Some("--bench") | Some("--bench-fast") => { bench(&args); return; }
        Some("--graph-dump") => {
            let db = helper::data_dir().join("index.db");
            let out = args.get(2).cloned().unwrap_or_else(|| "graph.json".into());
            if let Ok(c) = db::open(&db) {
                if let Ok(g) = graph::build(&c) {
                    let _ = std::fs::write(out, serde_json::to_vec_pretty(&g).unwrap_or_default());
                }
            }
            return;
        }
        Some("--backup-dump") => {
            let out = args.get(2).cloned().unwrap_or_else(|| "backups.json".into());
            if let Ok(c) = db::open(&helper::data_dir().join("index.db")) {
                if let Ok(r) = backups::report(&c) {
                    let _ = std::fs::write(out, serde_json::to_vec_pretty(&r).unwrap_or_default());
                }
            }
            return;
        }
        Some("--cleanup-dump") => {
            let out = args.get(2).cloned().unwrap_or_else(|| "cleanup.json".into());
            if let Ok(c) = db::open(&helper::data_dir().join("index.db")) {
                if let Ok(r) = cleanup::report(&c) {
                    let _ = std::fs::write(out, serde_json::to_vec_pretty(&r).unwrap_or_default());
                }
            }
            return;
        }
        Some("--archive-bench") => {
            // testing only: archive + restore a throwaway folder, report the time
            let (src, dir, out) = (args.get(2).cloned().unwrap_or_default(), args.get(3).cloned().unwrap_or_default(), args.get(4).cloned().unwrap_or_else(|| "bench.txt".into()));
            std::env::set_var("CARTOGRAPHER_ARCHIVE_DIR", &dir);
            std::env::set_var("CARTOGRAPHER_STATE_FILE", std::path::Path::new(&dir).join("bench-cleanup.json"));
            let quiet = |_: &str, _: u64, _: u64| {};
            let prog = cleanup::Progress { report: &quiet };
            let t = Instant::now();
            let res = match cleanup::archive(&src, "project", &prog) {
                Ok(a) => {
                    let arch = t.elapsed().as_secs_f64();
                    let t2 = Instant::now();
                    let r = cleanup::restore(&a.id, &prog);
                    format!("cores={} files={} bytes={} zip={} archive+verify={:.1}s restore={:.1}s restore_ok={}\n",
                        std::env::var("CARTOGRAPHER_ZIP_THREADS").unwrap_or_else(|_| "all".into()), a.files, a.bytes, a.zip_bytes, arch, t2.elapsed().as_secs_f64(), r.is_ok())
                }
                Err(e) => format!("FAIL {e}\n"),
            };
            let _ = std::fs::OpenOptions::new().create(true).append(true).open(out).and_then(|mut f| { use std::io::Write; f.write_all(res.as_bytes()) });
            return;
        }
        Some("--queue-selftest") => {
            let dir = args.get(2).cloned().unwrap_or_else(|| "selftest-archive".into());
            let out = args.get(3).cloned().unwrap_or_else(|| "queue-selftest.txt".into());
            let _ = std::fs::write(out, queue_selftest(&dir));
            return;
        }
        Some("--cleanup-selftest") => {
            let dir = args.get(2).cloned().unwrap_or_else(|| "selftest-archive".into());
            let out = args.get(3).cloned().unwrap_or_else(|| "selftest.txt".into());
            let _ = std::fs::write(out, cleanup::selftest(&dir));
            return;
        }
        Some("--mcp") => std::process::exit(ask::mcp_server()),
        Some("--ask") => {
            // testing: `--ask "question" out.json [auto|claude_code|api]`
            let q = args.get(2).cloned().unwrap_or_default();
            let out = args.get(3).cloned().unwrap_or_else(|| "ask.json".into());
            let backend = args.get(4).cloned().unwrap_or_else(|| "auto".into());
            let mut convo = ask::Conversation::default();
            let steps = std::sync::Mutex::new(Vec::new());
            let r = ask::ask(&helper::data_dir().join("index.db"), &mut convo, &q, &backend, &|s: &ask::Step| steps.lock().unwrap().push(s.tool.clone()));
            let v = match r { Ok(a) => serde_json::to_value(&a).unwrap_or_default(), Err(e) => serde_json::json!({ "error": e }) };
            let _ = std::fs::write(out, serde_json::to_vec_pretty(&v).unwrap_or_default());
            return;
        }
        Some("--tool") => {
            // testing: `--tool name '{json}' out.json`
            let name = args.get(2).cloned().unwrap_or_default();
            let input: serde_json::Value = serde_json::from_str(args.get(3).map(|s| s.as_str()).unwrap_or("{}")).unwrap_or_default();
            let out = args.get(4).cloned().unwrap_or_else(|| "tool.json".into());
            if let Ok(mut c) = db::open(&helper::data_dir().join("index.db")) {
                let t = Instant::now();
                let v = match ask_tools::run(&mut c, &name, &input) { Ok(v) => v, Err(e) => serde_json::json!({ "error": e }) };
                let _ = std::fs::write(out, serde_json::to_vec_pretty(&serde_json::json!({ "ms": t.elapsed().as_millis() as u64, "bytes": v.to_string().len(), "result": v })).unwrap_or_default());
            }
            return;
        }
        Some("--timeline-dump") => {
            // `--timeline-dump out.json <days> [db]`: testing; pass a copy of the index as db
            let out = args.get(2).cloned().unwrap_or_else(|| "timeline.json".into());
            let days: i64 = args.get(3).and_then(|d| d.parse().ok()).unwrap_or(7);
            let db = args.get(4).map(PathBuf::from).unwrap_or_else(|| helper::data_dir().join("index.db"));
            if let Ok(mut c) = db::open(&db) {
                let t = Instant::now();
                let r = history::report(&mut c, days).map(|r| serde_json::to_value(&r).unwrap_or_default());
                let v = match r { Ok(v) => serde_json::json!({ "ms": t.elapsed().as_millis() as u64, "report": v }), Err(e) => serde_json::json!({ "error": e.to_string() }) };
                let _ = std::fs::write(out, serde_json::to_vec_pretty(&v).unwrap_or_default());
            }
            return;
        }
        Some("--snapshot") => {
            // testing: `--snapshot <db> <scan_id> <time>` re-dates a scan and snapshots it (fake history on a copy)
            let db = PathBuf::from(args.get(2).cloned().unwrap_or_default());
            let scan: i64 = args.get(3).and_then(|d| d.parse().ok()).unwrap_or(0);
            let time: i64 = args.get(4).and_then(|d| d.parse().ok()).unwrap_or(0);
            if let Ok(mut c) = db::open(&db) {
                let _ = c.execute("UPDATE scans SET started = ?1 WHERE id = ?2", rusqlite::params![time, scan]);
                let _ = c.execute("UPDATE snapshots SET scan_id = -scan_id WHERE scan_id = ?1", [scan]); // keep the old one as history
                let _ = history::record(&mut c, scan);
            }
            return;
        }
        Some("--projects-dump") => {
            let out = args.get(2).cloned().unwrap_or_else(|| "projects.json".into());
            if let Ok(c) = db::open(&helper::data_dir().join("index.db")) {
                if let Ok(r) = projects::report(&c) {
                    let _ = std::fs::write(out, serde_json::to_vec_pretty(&r).unwrap_or_default());
                }
            }
            return;
        }
        Some("--health-dump") => {
            let out = args.get(2).cloned().unwrap_or_else(|| "health.json".into());
            if let Ok(c) = db::open(&helper::data_dir().join("index.db")) {
                if let Ok(r) = health::report(&c) {
                    let _ = std::fs::write(out, serde_json::to_vec_pretty(&r).unwrap_or_default());
                }
            }
            return;
        }
        Some("--helper") => std::process::exit(helper::run()),
        Some("--install-helper") => std::process::exit(helper::install()),
        Some("--uninstall-helper") => std::process::exit(helper::uninstall()),
        _ => {}
    }
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // a late screensaver start shows the 3D map, not the main window
            if matches!(screensaver::mode(args.get(1).map(|s| s.as_str())), Some(screensaver::Mode::Show)) {
                idle::open_show(app);
            } else {
                show_main(app);
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .setup(|app| {
            let dir = app.path().app_local_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db_path = dir.join("index.db");
            let conn = db::open(&db_path)?;
            let showing = Arc::new(AtomicBool::new(false));
            app.manage(AppState { db_path, conn: Mutex::new(conn), scanning: Arc::new(AtomicBool::new(false)), showing: showing.clone(), convo: Mutex::new(ask::Conversation::default()) });
            idle::start(app.handle().clone(), showing);
            watch::start(app.handle().clone(), begin_scan);
            {
                let db = app.state::<AppState>().db_path.clone();
                std::thread::spawn(move || {
                    if let Ok(mut c) = db::open(&db) {
                        history::backfill(&mut c);
                    }
                });
            }

            let open = MenuItem::with_id(app, "open", "Open Cartographer", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(app, &[&open, &sep, &quit])?;
            TrayIconBuilder::with_id("tray")
                .icon(app.default_window_icon().cloned().expect("icon"))
                .tooltip("Cartographer")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "open" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, e| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            if screensaver::only() {
                screensaver::request_now(); // the idle loop opens Show mode and quits when it closes
            } else if !std::env::args().any(|a| a == "--hidden") {
                show_main(app.handle());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![drives, scans, view, graph, health_report, get_settings, set_settings, show_status, show_preview, show_exit, backup_report, cleanup_report, cleanup_hide, cleanup_unhide_all, cleanup_run, cleanup_jobs, cleanup_ack, cleanup_unqueue, open_uninstall, load_layout, save_layout, reset_layout, is_scanning, start_scan, helper_status, set_fast_scan, reveal, open_path, timeline, projects_report, open_in_editor, open_terminal, open_url, ask_status, ask_set_key, ask_new, ask_question, screensaver_status, screensaver_install])
        .build(tauri::generate_context!())
        .expect("error while building Cartographer");

    app.run(|_app, event| {
        // Closing the window keeps Cartographer in the tray; only "Quit" exits.
        if let RunEvent::ExitRequested { api, code, .. } = event {
            if code.is_none() {
                api.prevent_exit();
            }
        }
    });
}
