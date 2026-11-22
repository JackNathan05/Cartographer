#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod db;
mod backups;
mod drives;
mod graph;
mod helper;
mod mft;
mod scan;

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
fn start_scan(app: AppHandle, state: State<AppState>, root: String) -> Result<(), String> {
    if state.scanning.swap(true, Ordering::SeqCst) {
        return Err("A scan is already running".into());
    }
    let flag = state.scanning.clone();
    let db_path = state.db_path.clone();
    std::thread::Builder::new()
        .name("scan".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let result = match fast_scan(&app, &root) {
                Some(r) => r,
                None => run_scan(&app, &db_path, &root),
            };
            flag.store(false, Ordering::SeqCst);
            match result {
                Ok(scan_id) => { let _ = app.emit("scan-done", ScanDone { root, scan_id }); }
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
        Some("--helper") => std::process::exit(helper::run()),
        Some("--install-helper") => std::process::exit(helper::install()),
        Some("--uninstall-helper") => std::process::exit(helper::uninstall()),
        _ => {}
    }
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .setup(|app| {
            let dir = app.path().app_local_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db_path = dir.join("index.db");
            let conn = db::open(&db_path)?;
            app.manage(AppState { db_path, conn: Mutex::new(conn), scanning: Arc::new(AtomicBool::new(false)) });

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

            if !std::env::args().any(|a| a == "--hidden") {
                show_main(app.handle());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![drives, scans, view, graph, backup_report, load_layout, save_layout, reset_layout, is_scanning, start_scan, helper_status, set_fast_scan, reveal, open_path])
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
