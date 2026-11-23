//! v0.7 Windows screensaver: Show mode as a real `.scr`, so Windows' own screensaver timer
//! (Settings > Lock screen > Screen saver) can start it instead of Cartographer's idle timer.
//!
//! A `.scr` is just this exe under another name. Windows runs it with:
//!   /s        show the screensaver   /p <hwnd>  small preview (we skip it)   /c  settings
//!
//! Windows runs a screensaver on its own separate desktop ("Screen-saver"), so a window from the
//! Cartographer already running in the tray (which lives on your normal desktop) can't be seen
//! there: it was just black. So for /s this process draws Show mode itself, on the screensaver
//! desktop, with its own WebView2 profile (a browser process shared with the tray app would
//! belong to the other desktop), and quits on the first key press or mouse move.

use crate::helper::data_dir;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// This process was started by Windows as the screensaver and should quit when Show mode closes.
pub static ONLY: AtomicBool = AtomicBool::new(false);

pub fn scr_path() -> PathBuf {
    data_dir().join("Cartographer.scr")
}

/// WebView2 profile for the screensaver process, separate from the tray app's.
pub fn webview_dir() -> PathBuf {
    data_dir().join("EBWebView-screensaver")
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

/// After Cartographer is updated, Cartographer.scr is still the old copy: refresh it (on startup
/// of the tray app). Skipped while the screensaver is running (the file is in use).
pub fn refresh_copy() {
    let scr = scr_path();
    let Ok(me) = std::env::current_exe() else { return };
    let (Ok(a), Ok(b)) = (std::fs::metadata(&me), std::fs::metadata(&scr)) else { return };
    if a.len() != b.len() || a.modified().ok() != b.modified().ok() {
        let _ = std::fs::copy(&me, &scr);
    }
}

fn desktop_key() -> Option<winreg::RegKey> {
    use winreg::enums::HKEY_CURRENT_USER;
    winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Control Panel\Desktop").ok()
}

/// Is Windows currently set to use Cartographer as its screensaver?
pub fn installed() -> bool {
    let Some(k) = desktop_key() else { return false };
    let v: String = k.get_value("SCRNSAVE.EXE").unwrap_or_default();
    let v = v.to_lowercase();
    // Windows may store the short 8.3 name (CARTOG~1.SCR)
    v.ends_with("cartographer.scr") || (v.ends_with(".scr") && v.rsplit('\\').next().map_or(false, |n| n.starts_with("cartog")))
}

/// Windows will start Cartographer as the screensaver by itself, so the tray app's own idle timer
/// should stay out of the way (it would open a second Show mode hidden behind the first).
pub fn windows_will_show() -> bool {
    let active = desktop_key().and_then(|k| k.get_value::<String, _>("ScreenSaveActive").ok()).map_or(false, |v| v.trim() == "1");
    active && installed()
}

pub fn only() -> bool {
    ONLY.load(Ordering::SeqCst)
}
