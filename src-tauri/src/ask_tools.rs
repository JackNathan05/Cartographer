//! v0.7 Ask Claude: the read-only tools Claude may use to look at the index.
//!
//! Every tool returns metadata only: folder / file / app names, paths, sizes and dates. Nothing
//! here reads file contents, and nothing changes anything. The same tools are offered over the
//! Anthropic API (ask.rs) and to Claude Code as a local MCP server (`cartographer --mcp`).

use crate::{backups, cleanup, graph, health, history, projects};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub fn gb(b: i64) -> String {
    let f = b as f64;
    if f >= 1e12 { format!("{:.2} TB", f / 1_099_511_627_776.0) }
    else if f >= 1e9 { format!("{:.1} GB", f / 1_073_741_824.0) }
    else if f >= 1e6 { format!("{:.0} MB", f / 1_048_576.0) }
    else { format!("{:.0} KB", f / 1024.0) }
}

/// Unix seconds -> "2026-11-23" (UTC; good enough for "when was this last touched").
pub fn day(t: i64) -> String {
    if t <= 0 {
        return "unknown".into();
    }
    let z = t.div_euclid(86400) + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

/// Tool definitions (name, description, JSON schema).
pub fn specs() -> Vec<Value> {
    vec![
        json!({
            "name": "overview",
            "description": "Start here. The PC at a glance: each drive's size, free space and scan age, the biggest top-level folders per drive, how many projects / games / apps were found, and backup status.",
            "input_schema": obj(json!({}), &[]),
        }),
        json!({
            "name": "find",
            "description": "Search the index by name. Returns matching folders (biggest first) and matching large files (1 MB or more), with size and last change date. Case-insensitive substring match on the folder or file name, e.g. \"airsim\" or \".iso\".",
            "input_schema": obj(json!({
                "query": { "type": "string", "description": "Part of a folder or file name" },
                "limit": { "type": "integer", "description": "Max results per kind, 1-50 (default 20)" }
            }), &["query"]),
        }),
        json!({
            "name": "folder",
            "description": "Look inside one folder: its total size, file count, last change, its biggest subfolders and its biggest files. Use a full Windows path like C:\\Users\\name\\Downloads or a drive root like E:\\.",
            "input_schema": obj(json!({ "path": { "type": "string", "description": "Full folder path" } }), &["path"]),
        }),
        json!({
            "name": "health",
            "description": "Health checks (takes ~15 s): backup freshness, free space per drive, Windows disk health, Recycle Bin / Temp / Desktop / Downloads clutter, unused items, scan age. Returns only the checks that need attention unless all_checks is true.",
            "input_schema": obj(json!({ "all_checks": { "type": "boolean", "description": "Include checks that are fine" } }), &[]),
        }),
        json!({
            "name": "projects",
            "description": "Every project folder: type (Rust, Python, ...), size, last worked on, git state (branch, uncommitted changes, unpushed commits, GitHub link) and which backups cover it.",
            "input_schema": obj(json!({}), &[]),
        }),
        json!({
            "name": "apps_and_games",
            "description": "Installed apps, games and services that Cartographer found, with size, install folder and last change. Optional name filter.",
            "input_schema": obj(json!({ "query": { "type": "string", "description": "Optional part of the name" } }), &[]),
        }),
        json!({
            "name": "cleanup_suggestions",
            "description": "Things that haven't been used in a while, with size, when they were last used and how Cartographer knows, whether they are backed up, and the suggested action (archive, delete to Recycle Bin, or uninstall). Also recent archive / delete actions.",
            "input_schema": obj(json!({}), &[]),
        }),
        json!({
            "name": "backups",
            "description": "Backup destinations (when each last ran, whether its drive is connected, what it covers) and folders that are not in any backup.",
            "input_schema": obj(json!({}), &[]),
        }),
        json!({
            "name": "changes",
            "description": "What changed recently: folders that grew, shrank, appeared or were deleted (from daily snapshots), apps and games installed or updated, and big files added or changed.",
            "input_schema": obj(json!({ "days": { "type": "integer", "description": "How many days back, e.g. 1, 7, 30 (0 = all history)" } }), &["days"]),
        }),
    ]
}

pub fn run(c: &mut Connection, name: &str, input: &Value) -> Result<Value, String> {
    let e = |x: rusqlite::Error| x.to_string();
    match name {
        "overview" => overview(c).map_err(e),
        "health" => {
            let all = input["all_checks"].as_bool().unwrap_or(false);
            let h = health::report(c).map_err(e)?;
            Ok(json!(h.checks.iter().filter(|x| all || x.status == "warn" || x.status == "bad").map(|x| json!({
                "area": x.area, "status": x.status, "title": x.title, "detail": x.detail,
            })).collect::<Vec<_>>()))
        }
        "find" => {
            let q = input["query"].as_str().unwrap_or("").trim().to_string();
            if q.is_empty() {
                return Err("query is empty".into());
            }
            let limit = input["limit"].as_i64().unwrap_or(20).clamp(1, 50);
            find(c, &q, limit).map_err(e)
        }
        "folder" => folder(c, input["path"].as_str().unwrap_or("")).map_err(e),
        "projects" => {
            let r = projects::report(c).map_err(e)?;
            Ok(json!(r.projects.iter().map(|p| json!({
                "name": p.name, "path": p.path, "size": gb(p.size), "last_change": day(p.newest),
                "type": p.tags, "about": p.description,
                "git": p.git.as_ref().map(|g| json!({
                    "branch": g.branch, "changed_files": g.changed, "new_files": g.untracked,
                    "unpushed_commits": g.ahead, "behind": g.behind,
                    "last_commit": day(g.last_commit), "last_message": g.last_message, "remote": g.remote,
                })),
                "backed_up_by": p.backups,
            })).collect::<Vec<_>>()))
        }
        "apps_and_games" => {
            let q = input["query"].as_str().unwrap_or("").to_lowercase();
            let g = graph::build(c).map_err(e)?;
            let mut v: Vec<&graph::Node> = g
                .nodes
                .iter()
                .filter(|n| matches!(n.kind, "app" | "game" | "service"))
                .filter(|n| q.is_empty() || n.label.to_lowercase().contains(&q))
                .collect();
            v.sort_by(|a, b| b.size.cmp(&a.size));
            Ok(json!(v.iter().take(120).map(|n| json!({
                "name": n.label, "kind": n.kind, "size": gb(n.size), "folder": n.path,
                "last_change": day(n.newest), "publisher": n.detail,
            })).collect::<Vec<_>>()))
        }
        "cleanup_suggestions" => {
            let r = cleanup::report(c).map_err(e)?;
            Ok(json!({
                "suggestions": r.suggestions.iter().take(60).map(|s| json!({
                    "name": s.name, "kind": s.kind, "path": s.path, "size": gb(s.size),
                    "last_used": day(s.last_used), "known_from": s.last_used_how,
                    "suggested_action": s.action, "why": s.reason, "backed_up_by": s.backed_up,
                })).collect::<Vec<_>>(),
                "recent_actions": r.actions.iter().rev().take(15).map(|a| json!({
                    "what": a.kind, "name": a.name, "from": a.from, "zip": a.to, "size": gb(a.bytes as i64),
                    "when": day(a.time), "restored": a.restored.is_some(),
                })).collect::<Vec<_>>(),
                "archive_folder": r.archive_dir,
            }))
        }
        "backups" => {
            let r = backups::report(c).map_err(e)?;
            Ok(json!({
                "destinations": r.destinations.iter().map(|d| json!({
                    "name": d.name, "target": d.target, "drive_connected": d.connected,
                    "last_activity": day(d.last_activity), "stored": gb(d.stored),
                    "covers": d.sources.iter().map(|s| json!({ "path": s.path, "size": gb(s.size) })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "folders_not_backed_up": r.folders.iter().filter(|f| f.backups.is_empty()).take(40).map(|f| json!({
                    "path": f.path, "size": gb(f.size), "last_change": day(f.newest), "note": f.hint,
                })).collect::<Vec<_>>(),
            }))
        }
        "changes" => {
            let days = input["days"].as_i64().unwrap_or(7).clamp(0, 3650);
            let r = history::report(c, days).map_err(e)?;
            Ok(json!({
                "history_since": r.history_start.map(day),
                "drives": r.drives.iter().map(|d| json!({
                    "drive": d.root, "compared_with": d.since.map(day), "change": gb(d.change.abs()),
                    "direction": if d.change >= 0 { "grew" } else { "shrank" },
                })).collect::<Vec<_>>(),
                "folders": r.changes.iter().take(40).map(|ch| json!({
                    "path": ch.path, "what": ch.kind, "before": gb(ch.before), "after": gb(ch.after),
                })).collect::<Vec<_>>(),
                "installed_or_updated": r.installs.iter().take(40).map(|i| json!({
                    "name": i.name, "kind": i.kind, "date": day(i.when), "size": if i.size > 0 { Some(gb(i.size)) } else { None },
                })).collect::<Vec<_>>(),
                "big_files": r.files.iter().take(25).map(|f| json!({
                    "file": format!("{}\\{}", f.dir, f.name), "size": gb(f.size), "modified": day(f.modified),
                })).collect::<Vec<_>>(),
            }))
        }
        other => Err(format!("unknown tool {other}")),
    }
}

fn overview(c: &Connection) -> rusqlite::Result<Value> {
    let scans = crate::db::scans(c)?;
    let now = cleanup::now();
    let mut drives = Vec::new();
    for d in crate::drives::list() {
        let sc = scans.iter().find(|s| s.root.to_uppercase().starts_with(&format!("{}:", d.letter)));
        let mut top = Vec::new();
        if let Some(s) = sc {
            let mut q = c.prepare_cached("SELECT path, size FROM dirs WHERE parent_id = ?1 ORDER BY size DESC LIMIT 8")?;
            let rows = q.query_map([s.root_dir], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            for r in rows.flatten() {
                top.push(json!({ "path": r.0, "size": gb(r.1) }));
            }
        }
        drives.push(json!({
            "drive": format!("{}:", d.letter), "label": d.label, "kind": d.kind, "size": gb(d.total),
            "free": gb(d.free), "free_percent": if d.total > 0 { d.free * 100 / d.total } else { 0 },
            "scanned_hours_ago": sc.map(|s| (now - s.started) / 3600), "biggest_folders": top,
        }));
    }
    let g = graph::build(c)?;
    let count = |k: &str| g.nodes.iter().filter(|n| n.kind == k).count();
    let b = backups::report(c)?;
    Ok(json!({
        "today": day(now),
        "drives": drives,
        "found": { "projects": count("project"), "games": count("game"), "apps": count("app"), "services": count("service") },
        "backups": b.destinations.iter().map(|d| json!({ "name": d.name, "last_activity": day(d.last_activity), "connected": d.connected })).collect::<Vec<_>>(),
    }))
}

fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

fn find(c: &Connection, q: &str, limit: i64) -> rusqlite::Result<Value> {
    let pat = format!("%{}%", like_escape(q));
    let mut s = c.prepare("SELECT path, size, files, newest FROM dirs WHERE name LIKE ?1 ESCAPE '\\' ORDER BY size DESC LIMIT ?2")?;
    let folders: Vec<Value> = s
        .query_map(params![pat, limit], |r| {
            Ok(json!({ "path": r.get::<_, String>(0)?, "size": gb(r.get(1)?), "files": r.get::<_, i64>(2)?, "last_change": day(r.get(3)?) }))
        })?
        .flatten()
        .collect();
    let mut s = c.prepare(
        "SELECT d.path, f.name, f.size, f.modified FROM files f JOIN dirs d ON d.id = f.dir_id
         WHERE f.name LIKE ?1 ESCAPE '\\' ORDER BY f.size DESC LIMIT ?2",
    )?;
    let files: Vec<Value> = s
        .query_map(params![pat, limit], |r| {
            Ok(json!({ "file": format!("{}\\{}", r.get::<_, String>(0)?, r.get::<_, String>(1)?), "size": gb(r.get(2)?), "modified": day(r.get(3)?) }))
        })?
        .flatten()
        .collect();
    Ok(json!({ "folders": folders, "large_files": files }))
}

fn folder(c: &Connection, path: &str) -> rusqlite::Result<Value> {
    let k = graph::key(path);
    let found = graph::lookup(c, &[k.clone()].into_iter().collect())?;
    let Some(d) = found.get(&k) else {
        return Ok(json!({ "error": format!("{path} is not in the index (not scanned, or the path is wrong). Try find.") }));
    };
    let (files, dirs): (i64, i64) = c.query_row("SELECT files, dirs FROM dirs WHERE id = ?1", [d.id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let mut s = c.prepare("SELECT name, size, files, newest FROM dirs WHERE parent_id = ?1 ORDER BY size DESC LIMIT 30")?;
    let subs: Vec<Value> = s
        .query_map([d.id], |r| {
            Ok(json!({ "name": r.get::<_, String>(0)?, "size": gb(r.get(1)?), "files": r.get::<_, i64>(2)?, "last_change": day(r.get(3)?) }))
        })?
        .flatten()
        .collect();
    let mut s = c.prepare("SELECT name, size, modified FROM files WHERE dir_id = ?1 ORDER BY size DESC LIMIT 15")?;
    let big: Vec<Value> = s
        .query_map([d.id], |r| Ok(json!({ "name": r.get::<_, String>(0)?, "size": gb(r.get(1)?), "modified": day(r.get(2)?) })))?
        .flatten()
        .collect();
    Ok(json!({
        "path": d.path, "size": gb(d.size), "files": files, "subfolders": dirs, "last_change": day(d.newest),
        "biggest_subfolders": subs, "biggest_files": big,
        "note": "Files under 1 MB are counted in sizes but not listed one by one.",
    }))
}
