//! v0.5 live updates: watch every NTFS drive for changes and quietly rescan a drive in the
//! background when something real changed. At most once per `refresh_minutes` per drive, and
//! only after a minute of no keyboard / mouse input, so it never competes with what you're doing.
//! Noise (Temp, browser caches, Windows, the Recycle Bin, Cartographer's own files) is ignored.

use crate::helper::data_dir;
use crate::idle;
use notify::{RecursiveMode, Watcher};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::AppHandle;

pub(crate) fn noise(path_lower: &str, own: &str) -> bool {
    const SKIP: &[&str] = &[
        "\\appdata\\local\\temp", "\\$recycle.bin", "\\windows\\", "\\system volume information", "\\$extend",
        "\\programdata\\microsoft", "\\appdata\\local\\packages", "\\appdata\\local\\microsoft", "\\appdata\\roaming\\microsoft",
        "\\cache\\", "\\code cache", "\\gpucache", "\\cachestorage", "\\service worker", "\\user data\\", "\\crashpad",
        "\\logs\\", "\\.git\\objects", "pagefile.sys", "swapfile.sys", "hiberfil.sys", "\\node_modules\\.cache",
    ];
    path_lower.starts_with(own) || SKIP.iter().any(|s| path_lower.contains(s))
}

pub fn start(app: AppHandle, kick: fn(&AppHandle, String) -> Result<(), String>) {
    std::thread::spawn(move || {
        let dirty: Arc<Mutex<HashMap<String, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
        let own = data_dir().to_string_lossy().to_lowercase();
        let mut watchers = Vec::new();
        for d in crate::drives::list().into_iter().filter(|d| d.kind == "fixed" && d.fs == "NTFS") {
            let letter = d.letter.clone();
            let (dirty, own) = (dirty.clone(), own.clone());
            let w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                let real = match &res {
                    Ok(ev) => ev.paths.iter().any(|p| !noise(&p.to_string_lossy().to_lowercase(), &own)),
                    Err(_) => true, // the event buffer overflowed: assume something changed
                };
                if real {
                    dirty.lock().unwrap().entry(letter.clone()).or_insert_with(Instant::now);
                }
            });
            if let Ok(mut w) = w {
                if w.watch(std::path::Path::new(&format!("{}:\\", d.letter)), RecursiveMode::Recursive).is_ok() {
                    watchers.push(w);
                }
            }
        }

        loop {
            std::thread::sleep(Duration::from_secs(60));
            let s = idle::load();
            if !s.auto_refresh || idle::idle_secs() < 60 {
                continue;
            }
            let due: Vec<String> = {
                let d = dirty.lock().unwrap();
                d.keys().cloned().collect()
            };
            if due.is_empty() {
                continue;
            }
            let db = data_dir().join("index.db");
            let Ok(conn) = crate::db::open(&db) else { continue };
            let scans = crate::db::scans(&conn).unwrap_or_default();
            let now = crate::cleanup::now();
            for letter in due {
                let Some(scan) = scans.iter().find(|x| x.root.to_uppercase().starts_with(&format!("{letter}:"))) else {
                    dirty.lock().unwrap().remove(&letter); // never scanned: you decide when
                    continue;
                };
                if now - scan.started < s.refresh_minutes as i64 * 60 {
                    continue;
                }
                if kick(&app, format!("{letter}:\\")).is_ok() {
                    dirty.lock().unwrap().remove(&letter);
                    break; // one drive at a time
                }
            }
        }
        #[allow(unreachable_code)]
        drop(watchers);
    });
}
