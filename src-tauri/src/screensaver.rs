//! v0.7 Windows screensaver: Show mode as a real `.scr`, so Windows' own screensaver timer
//! (Settings > Lock screen > Screen saver) can start it instead of Cartographer's idle timer.
//!
//! A `.scr` is just this exe under another name. Windows runs it with:
//!   /s        show the screensaver   /p <hwnd>  small preview (we skip it)   /c  settings
//! For /s, the `.scr` asks the Cartographer already running in the tray to open Show mode (a request
//! file it checks every 2 s) and stays alive until Show mode closes, which is what Windows expects.
//! If Cartographer isn't running, the `.scr` starts it in screensaver-only mode and quits afterwards.

use crate::helper::data_dir;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// This process was started by Windows as the screensaver and should quit when Show mode closes.
pub static ONLY: AtomicBool = AtomicBool::new(false);

fn request_file() -> PathBuf {
    data_dir().join("screensaver-request")
}
pub fn active_file() -> PathBuf {
    data_dir().join("screensaver-active")
}

pub fn scr_path() -> PathBuf {
    data_dir().join("Cartographer.scr")
}

/// What Windows asked for, from the first argument ("/s", "/S", "/p 1234", "/c:5678").
pub enum Mode {
    Show,
    Preview,
    Configure,
}

pub fn mode(arg: Option<&str>) -> Option<Mode> {
    let a = arg?.to_lowercase();
    if a.starts_with("/s") || a.starts_with("-s") {
        Some(Mode::Show)
    } else if a.starts_with("/p") || a.starts_with("-p") {
        Some(Mode::Preview)
    } else if a.starts_with("/c") || a.starts_with("-c") {
        Some(Mode::Configure)
    } else {
        None
    }
}

/// `/s`: hand off to the running app and wait while Show mode is up. Returns false when no
/// running Cartographer answered, so the caller starts one in screensaver-only mode.
pub fn hand_off() -> bool {
    let _ = std::fs::create_dir_all(data_dir());
    let _ = std::fs::remove_file(active_file());
    if std::fs::write(request_file(), crate::cleanup::now().to_string()).is_err() {
        return false;
    }
    let t0 = Instant::now();
    while !active_file().exists() {
        if t0.elapsed() > Duration::from_secs(6) {
            let _ = std::fs::remove_file(request_file());
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    // Show mode is up: stay alive until it closes (or 6 h, as a safety net)
    while active_file().exists() && t0.elapsed() < Duration::from_secs(6 * 3600) {
        std::thread::sleep(Duration::from_millis(500));
    }
    true
}

/// Called from the idle loop: was a fresh screensaver request left for us?
pub fn take_request() -> bool {
    let p = request_file();
    let Ok(s) = std::fs::read_to_string(&p) else { return false };
    let _ = std::fs::remove_file(&p);
    let t: i64 = s.trim().parse().unwrap_or(0);
    crate::cleanup::now() - t < 15
}

pub fn set_active(on: bool) {
    if on {
        let _ = std::fs::write(active_file(), b"1");
    } else {
        let _ = std::fs::remove_file(active_file());
    }
}

/// Copies this exe to Cartographer.scr and opens Windows' Screen Saver Settings with it selected;
/// you confirm there (Windows' own dialog makes the actual change).
pub fn install() -> Result<String, String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let scr = scr_path();
    std::fs::create_dir_all(data_dir()).map_err(|e| e.to_string())?;
    std::fs::copy(&me, &scr).map_err(|e| format!("Couldn't create the screensaver file: {e}"))?;
    std::process::Command::new("rundll32.exe")
        .arg("desk.cpl,InstallScreenSaver")
        .arg(&scr)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(scr.to_string_lossy().into_owned())
}

/// Is Windows currently set to use Cartographer as its screensaver?
pub fn installed() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    let Ok(k) = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Control Panel\Desktop") else { return false };
    let v: String = k.get_value("SCRNSAVE.EXE").unwrap_or_default();
    let v = v.to_lowercase();
    // Windows may store the short 8.3 name (CARTOG~1.SCR)
    v.ends_with("cartographer.scr") || (v.ends_with(".scr") && v.rsplit('\\').next().map_or(false, |n| n.starts_with("cartog")))
}

pub fn only() -> bool {
    ONLY.load(Ordering::SeqCst)
}

/// Leaves a request for the idle loop (used when this process is the screensaver itself).
pub fn request_now() {
    let _ = std::fs::create_dir_all(data_dir());
    let _ = std::fs::write(request_file(), crate::cleanup::now().to_string());
}
