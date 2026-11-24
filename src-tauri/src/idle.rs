//! Show mode: after N minutes with no keyboard / mouse input, open a fullscreen window with
//! the cinematic 3D map. Any input closes it. Never starts while a fullscreen app or game is
//! running, the screen is locked, OBS is open, or the PC is on battery. Stops on its own after
//! M minutes so the monitor and PC still sleep normally (we never block Windows' power settings).
//!
//! The same loop tells the UI when OBS opens / closes (for automatic Privacy mode).

use crate::helper::data_dir;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows_sys::Win32::UI::Shell::SHQueryUserNotificationState;

#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    #[serde(default = "yes")]
    pub show_enabled: bool,
    #[serde(default = "ten")]
    pub idle_minutes: u32,
    #[serde(default = "twenty")]
    pub show_minutes: u32,
    #[serde(default)]
    pub privacy: bool,
    #[serde(default = "yes")]
    pub privacy_with_obs: bool,
    #[serde(default = "yes")]
    pub auto_refresh: bool,
    #[serde(default = "thirty")]
    pub refresh_minutes: u32,
    /// Ask Claude: the user agreed to send index metadata to Claude
    #[serde(default)]
    pub ask_ok: bool,
    /// "auto" (Claude Code when installed, else API key) | "claude_code" | "api"
    #[serde(default = "auto")]
    pub ask_backend: String,
}
fn auto() -> String { "auto".into() }
fn thirty() -> u32 { 30 }
fn yes() -> bool { true }
fn ten() -> u32 { 10 }
fn twenty() -> u32 { 20 }

impl Default for Settings {
    fn default() -> Self {
        Settings { show_enabled: true, idle_minutes: 10, show_minutes: 20, privacy: false, privacy_with_obs: true, auto_refresh: true, refresh_minutes: 30, ask_ok: false, ask_backend: auto() }
    }
}

fn path() -> std::path::PathBuf {
    data_dir().join("settings.json")
}

pub fn load() -> Settings {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(s: &Settings) -> Result<(), String> {
    std::fs::create_dir_all(data_dir()).map_err(|e| e.to_string())?;
    std::fs::write(path(), serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// When Cartographer is the Windows screensaver: quit the instant you're back. Windows only
/// returns to your desktop once the screensaver process has ended, so this ends it right away
/// (no window teardown) on any key, click or scroll, or a mouse move of more than a few pixels
/// (a bumped desk doesn't count). Checks every 30 ms.
pub fn quit_on_input() {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    std::thread::spawn(|| {
        let last_input = || {
            let mut li = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
            unsafe { GetLastInputInfo(&mut li) };
            li.dwTime
        };
        let cursor = || {
            let mut p = POINT { x: 0, y: 0 };
            unsafe { GetCursorPos(&mut p) };
            (p.x, p.y)
        };
        // troubleshooting: create "scr-debug" in the data folder to log what this loop sees
        let dbg = data_dir().join("scr-debug").exists();
        let t_start = Instant::now();
        let log = |m: String| {
            if dbg {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(data_dir().join("scr-debug.log")) {
                    let _ = writeln!(f, "{:>7} ms  {m}", t_start.elapsed().as_millis());
                }
            }
        };
        std::thread::sleep(Duration::from_millis(1000)); // let the desktop switch settle
        let base = cursor();
        let (mut tick, mut prev) = (last_input(), base);
        log(format!("watching: last input {tick}, cursor {base:?}"));
        loop {
            std::thread::sleep(Duration::from_millis(30));
            let t = last_input();
            if t == tick {
                continue;
            }
            let cur = cursor();
            let from_base = (((cur.0 - base.0) as f64).powi(2) + ((cur.1 - base.1) as f64).powi(2)).sqrt();
            log(format!("input at {t} (now {}): cursor {cur:?}, {from_base:.0} px from start", unsafe { GetTickCount() }));
            // input with the pointer not moving = a key, click or scroll
            if cur == prev || from_base > 8.0 {
                log("quitting".into());
                crate::screensaver::quit_now();
            }
            (tick, prev) = (t, cur);
        }
    });
}

/// Seconds since the last keyboard / mouse input anywhere on the PC.
pub fn idle_secs() -> u64 {
    let mut li = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    unsafe {
        if GetLastInputInfo(&mut li) == 0 {
            return 0;
        }
        (GetTickCount().wrapping_sub(li.dwTime) / 1000) as u64
    }
}

fn process_running(names: &[&str]) -> bool {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 {
            return false;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                let name = String::from_utf16_lossy(&e.szExeFile[..n]).to_lowercase();
                if names.contains(&name.as_str()) {
                    found = true;
                    break;
                }
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        windows_sys::Win32::Foundation::CloseHandle(snap);
        found
    }
}

pub fn obs_running() -> bool {
    process_running(&["obs64.exe", "obs32.exe", "obs.exe"])
}

/// Why show mode shouldn't start right now (None = OK to start).
pub fn blocked_reason() -> Option<&'static str> {
    let mut state = 0i32;
    if unsafe { SHQueryUserNotificationState(&mut state) } == 0 {
        match state {
            1 => return Some("screen is locked"),
            2 | 3 => return Some("a fullscreen app or game is running"),
            4 => return Some("presentation mode is on"),
            _ => {}
        }
    }
    if obs_running() {
        return Some("OBS is open");
    }
    let mut ps: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    if unsafe { GetSystemPowerStatus(&mut ps) } != 0 && ps.ACLineStatus == 0 && ps.BatteryFlag != 128 {
        return Some("running on battery");
    }
    None
}

pub fn open_show(app: &AppHandle) {
    if app.get_webview_window("show").is_some() {
        return;
    }
    let mut b = WebviewWindowBuilder::new(app, "show", WebviewUrl::App("index.html?mode=show".into()));
    if crate::screensaver::only() {
        // own browser process, so it draws on the screensaver desktop this process runs on
        b = b.data_directory(crate::screensaver::webview_dir());
    }
    let _ = b
        .title("Cartographer")
        .fullscreen(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(true)
        .background_color(tauri::window::Color(7, 10, 18, 255))
        .build();
}

pub fn close_show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("show") {
        let _ = w.close();
    }
}

/// Background loop (runs for the life of the app; ~0% CPU: one check every 2 s).
pub fn start(app: AppHandle, showing: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut shown_at: Option<Instant> = None;
        let mut idle_at_open = 0u64;
        let mut wait_for_input = false; // after a show times out, don't restart until you're back
        let mut last_obs = false;
        let only = crate::screensaver::only();
        if only {
            // started by Windows as the screensaver: show right away, until you're back
            open_show(&app);
            shown_at = Some(Instant::now());
            idle_at_open = idle_secs();
            showing.store(true, Ordering::SeqCst);
            quit_on_input();
        }
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let s = load();
            let idle = idle_secs();

            let obs = obs_running();
            if obs != last_obs {
                last_obs = obs;
                let _ = app.emit("obs-changed", obs);
            }

            if let Some(t) = shown_at {
                let window_gone = app.get_webview_window("show").is_none();
                let input = idle < idle_at_open.saturating_sub(1) || idle < 2;
                let timed_out = !only && t.elapsed() >= Duration::from_secs(s.show_minutes as u64 * 60);
                if window_gone || input || timed_out {
                    close_show(&app);
                    shown_at = None;
                    showing.store(false, Ordering::SeqCst);
                    wait_for_input = timed_out && !input;
                    if only {
                        crate::screensaver::quit_now(); // started only as the screensaver
                    }
                }
                continue;
            }
            if wait_for_input {
                if idle < 5 {
                    wait_for_input = false;
                }
                continue;
            }
            if s.show_enabled
                && idle >= s.idle_minutes as u64 * 60
                && !crate::screensaver::windows_will_show() // Windows starts Cartographer.scr itself
                && blocked_reason().is_none()
            {
                open_show(&app);
                shown_at = Some(Instant::now());
                idle_at_open = idle;
                showing.store(true, Ordering::SeqCst);
            }
        }
    });
}
