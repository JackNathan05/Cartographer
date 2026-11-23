//! v0.5 Health: one list of checks, each ok / warn / bad / info, with a suggested action.
//! Replaces the monthly upkeep report: backups, free space, drive health, clutter,
//! how fresh the map is, and whether your projects are backed up.

use crate::graph::{key, norm};
use crate::{backups, cleanup, drives, idle};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::os::windows::process::CommandExt;
use std::path::Path;

const GB: i64 = 1 << 30;
const DAY: i64 = 86_400;

#[derive(Serialize)]
pub struct Check {
    pub area: &'static str,   // Backups | Storage | Drives | Clutter | Map
    pub status: &'static str, // ok | warn | bad | info
    pub title: String,
    pub detail: String,
    pub action: Option<&'static str>, // backups | cleanup | space:<letter> | rescan:<letter> | open:<path> | fastscan
    pub target: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Disk {
    #[serde(rename = "FriendlyName")]
    pub name: String,
    #[serde(rename = "MediaType", default)]
    pub media: serde_json::Value,
    #[serde(rename = "BusType", default)]
    pub bus: serde_json::Value,
    #[serde(rename = "HealthStatus", default)]
    pub health: serde_json::Value,
    #[serde(rename = "OperationalStatus", default)]
    pub op: serde_json::Value,
    #[serde(rename = "Size", default)]
    pub size: i64,
    #[serde(rename = "DeviceId", default)]
    pub id: serde_json::Value,
    #[serde(default)]
    pub letters: Vec<String>,
}

#[derive(Serialize)]
pub struct Report {
    pub checks: Vec<Check>,
    pub disks: Vec<Disk>,
    pub generated: i64,
}

fn dir_size(p: &Path, max_entries: usize) -> i64 {
    walkdir::WalkDir::new(p)
        .follow_links(false)
        .into_iter()
        .take(max_entries)
        .flatten()
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len() as i64)
        .sum()
}

fn val_str(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// Physical disks with Windows' own health verdict (Get-PhysicalDisk), mapped to drive letters.
pub fn disks() -> Vec<Disk> {
    let ps = "$d=Get-PhysicalDisk | Select-Object DeviceId,FriendlyName,MediaType,BusType,HealthStatus,OperationalStatus,Size; \
              $p=Get-Partition | Where-Object DriveLetter | Select-Object DiskNumber,DriveLetter; \
              @{disks=@($d);parts=@($p)} | ConvertTo-Json -Depth 4 -Compress";
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", ps])
        .creation_flags(0x0800_0000)
        .output();
    let Ok(out) = out else { return vec![] };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else { return vec![] };
    let mut disks: Vec<Disk> = serde_json::from_value(v["disks"].clone()).unwrap_or_default();
    if let Some(parts) = v["parts"].as_array() {
        for p in parts {
            let num = val_str(&p["DiskNumber"]);
            let letter = val_str(&p["DriveLetter"]);
            if let Some(d) = disks.iter_mut().find(|d| val_str(&d.id) == num) {
                if !letter.is_empty() {
                    d.letters.push(letter);
                }
            }
        }
    }
    for d in disks.iter_mut() {
        d.letters.sort();
    }
    disks
}

pub fn report(conn: &Connection) -> rusqlite::Result<Report> {
    let now = cleanup::now();
    let mut checks = Vec::new();
    let profile = norm(&std::env::var("USERPROFILE").unwrap_or_default());

    // ---- backups
    let b = backups::report(conn)?;
    if b.destinations.is_empty() {
        checks.push(Check { area: "Backups", status: "warn", title: "No backups set up".into(),
            detail: format!("Add your backups to {} so Cartographer can check them.", b.config_path), action: Some("backups"), target: None });
    }
    for d in &b.destinations {
        let age = if d.last_activity > 0 { now - d.last_activity } else { i64::MAX };
        let (status, title) = if !d.connected {
            ("warn", format!("{} isn't connected", d.name))
        } else if !d.target_exists {
            ("bad", format!("{}: backup folder missing", d.name))
        } else if age < 8 * DAY {
            ("ok", format!("{} is up to date", d.name))
        } else if age < 31 * DAY {
            ("warn", format!("{} is getting old", d.name))
        } else {
            ("bad", format!("{} is out of date", d.name))
        };
        let detail = if d.last_activity > 0 {
            format!("Last activity {} days ago · {} on {}:", age / DAY, d.target, d.drive)
        } else {
            format!("No activity found · {}", d.target)
        };
        checks.push(Check { area: "Backups", status, title, detail, action: Some("backups"), target: None });
    }
    let risky: Vec<&backups::Folder> = b
        .folders
        .iter()
        .filter(|f| f.backups.is_empty() && !f.hint.as_deref().unwrap_or("").contains("reinstallable") && !f.hint.as_deref().unwrap_or("").contains("re-download") && !f.hint.as_deref().unwrap_or("").contains("partly"))
        .collect();
    if !risky.is_empty() {
        let bytes: i64 = risky.iter().map(|f| f.size).sum();
        let names: Vec<String> = risky.iter().take(3).map(|f| f.label.clone()).collect();
        checks.push(Check { area: "Backups", status: "warn",
            title: format!("{} of your folders aren't backed up ({:.1} GB)", risky.len(), bytes as f64 / GB as f64),
            detail: format!("{}{}", names.join(", "), if risky.len() > 3 { ", …" } else { "" }), action: Some("backups"), target: None });
    } else if !b.destinations.is_empty() {
        checks.push(Check { area: "Backups", status: "ok", title: "All of your own folders are backed up".into(), detail: String::new(), action: None, target: None });
    }

    // ---- storage
    for d in drives::list().into_iter().filter(|d| d.kind == "fixed" && d.total > 0) {
        let pct = d.free as f64 / d.total as f64;
        let status = if pct < 0.10 || d.free < 15 * GB { "bad" } else if pct < 0.20 || d.free < 30 * GB { "warn" } else { "ok" };
        checks.push(Check {
            area: "Storage", status,
            title: format!("{}: {:.0} GB free ({:.0}%)", d.letter, d.free as f64 / GB as f64, pct * 100.0),
            detail: format!("{} · {:.0} GB total", if d.label.is_empty() { "Local disk" } else { &d.label }, d.total as f64 / GB as f64),
            action: Some("space"), target: Some(d.letter.clone()),
        });
    }

    // ---- physical drives
    let disks = disks();
    for d in &disks {
        let h = val_str(&d.health);
        let status = match h.as_str() { "Healthy" => "ok", "Warning" => "warn", "Unhealthy" => "bad", _ => "info" };
        checks.push(Check {
            area: "Drives", status,
            title: format!("{} · {}", d.name.trim(), if h.is_empty() { "unknown" } else { &h }),
            detail: format!("{} {} · {:.0} GB{}", val_str(&d.media).replace("Unspecified", "Drive"), val_str(&d.bus), d.size as f64 / 1e9,
                if d.letters.is_empty() { String::new() } else { format!(" · {}", d.letters.iter().map(|l| format!("{l}:")).collect::<Vec<_>>().join(" ")) }),
            action: None, target: None,
        });
    }

    // ---- clutter
    let sid_bins: i64 = {
        let mut total = 0;
        for d in drives::list().into_iter().filter(|d| d.kind == "fixed") {
            let root = format!("{}:\\$Recycle.Bin", d.letter);
            if let Ok(rd) = std::fs::read_dir(&root) {
                for e in rd.flatten() {
                    total += dir_size(&e.path(), 200_000);
                }
            }
        }
        total
    };
    checks.push(Check {
        area: "Clutter", status: if sid_bins > 10 * GB { "warn" } else { "info" },
        title: format!("Recycle Bin: {:.1} GB", sid_bins as f64 / GB as f64),
        detail: "Emptying it is permanent; do it from the desktop when you're sure.".into(), action: None, target: None,
    });
    let temp = std::env::var("TEMP").unwrap_or_default();
    let temp_size = dir_size(Path::new(&temp), 300_000);
    checks.push(Check {
        area: "Clutter", status: if temp_size > 5 * GB { "warn" } else { "info" },
        title: format!("Temp files: {:.1} GB", temp_size as f64 / GB as f64),
        detail: temp.clone(), action: Some("open"), target: Some(temp),
    });
    let desktop = format!("{profile}\\Desktop");
    let public_desktop = format!("{}\\Users\\Public\\Desktop", std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()));
    let mut items = 0usize;
    let mut shortcuts = 0usize;
    for dsk in [&desktop, &public_desktop] {
        if let Ok(rd) = std::fs::read_dir(dsk) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_lowercase();
                if n == "desktop.ini" {
                    continue;
                }
                items += 1;
                if n.ends_with(".lnk") || n.ends_with(".url") {
                    shortcuts += 1;
                }
            }
        }
    }
    checks.push(Check {
        area: "Clutter", status: if items > 20 { "warn" } else { "info" },
        title: format!("Desktop: {items} items ({shortcuts} shortcuts)"),
        detail: "App updates often add shortcuts back.".into(), action: Some("open"), target: Some(desktop),
    });
    let c = cleanup::report(conn)?;
    let dl: Vec<&cleanup::Suggestion> = c.suggestions.iter().filter(|s| s.kind == "download").collect();
    if !dl.is_empty() {
        let bytes: i64 = dl.iter().map(|s| s.size).sum();
        checks.push(Check { area: "Clutter", status: if bytes > GB { "warn" } else { "info" },
            title: format!("{} old Downloads ({:.1} GB)", dl.len(), bytes as f64 / GB as f64), detail: "Older than 60 days".into(),
            action: Some("cleanup"), target: None });
    }
    if !c.suggestions.is_empty() {
        let bytes: i64 = c.suggestions.iter().map(|s| s.size).sum();
        checks.push(Check { area: "Clutter", status: "info",
            title: format!("{} things you haven't used in a while ({:.1} GB)", c.suggestions.len(), bytes as f64 / GB as f64),
            detail: "Archive, delete or uninstall from Cleanup; everything can be undone.".into(), action: Some("cleanup"), target: None });
    }

    // ---- map freshness
    let scans = crate::db::scans(conn)?;
    for d in drives::list().into_iter().filter(|d| d.kind == "fixed") {
        match scans.iter().find(|s| s.root.to_uppercase().starts_with(&format!("{}:", d.letter))) {
            Some(s) => {
                let age = now - s.started;
                checks.push(Check {
                    area: "Map", status: if age > 14 * DAY { "warn" } else { "ok" },
                    title: format!("{}: scanned {}", d.letter, if age < 3600 { "within the hour".into() } else if age < DAY { format!("{} h ago", age / 3600) } else { format!("{} days ago", age / DAY) }),
                    detail: format!("{} files · {}", s.files, if s.method == "fast" { "fast scan" } else { "folder walk" }),
                    action: Some("rescan"), target: Some(d.letter.clone()),
                });
            }
            None => checks.push(Check { area: "Map", status: "info", title: format!("{}: not scanned yet", d.letter),
                detail: String::new(), action: Some("rescan"), target: Some(d.letter.clone()) }),
        }
    }
    if !crate::helper::installed() {
        checks.push(Check { area: "Map", status: "info", title: "Fast scan is off".into(),
            detail: "Scans use the slower folder walk. Turn it on in the sidebar (one Windows prompt).".into(), action: None, target: None });
    }
    let s = idle::load();
    checks.push(Check { area: "Map", status: "info",
        title: if s.auto_refresh { format!("Map keeps itself up to date (at most every {} min)", s.refresh_minutes) } else { "Automatic map updates are off".into() },
        detail: String::new(), action: None, target: None });
    let _ = key;

    Ok(Report { checks, disks, generated: now })
}
