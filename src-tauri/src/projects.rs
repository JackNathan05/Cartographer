//! v0.6 Project cards: every project the map finds, with what kind of project it is, its git
//! state (branch, uncommitted changes, unpushed commits, last commit), size, last edit and backups.
//! Git is asked directly (`git status` / `git log`), all projects at once on every core.

use crate::graph;
use rayon::prelude::*;
use rusqlite::Connection;
use serde::Serialize;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const NO_WINDOW: u32 = 0x0800_0000;

#[derive(Serialize, Default)]
pub struct Git {
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: i64,
    pub behind: i64,
    pub changed: i64,
    pub untracked: i64,
    pub last_commit: i64,
    pub last_message: String,
    pub remote: Option<String>, // web link when it's on GitHub / GitLab / similar
}

#[derive(Serialize)]
pub struct Project {
    pub name: String,
    pub path: String,
    pub size: i64,
    pub newest: i64,
    pub scan_id: Option<i64>,
    pub dir_id: Option<i64>,
    pub backups: Vec<String>,
    pub tags: Vec<String>,
    pub description: Option<String>,
    pub git: Option<Git>,
}

#[derive(Serialize)]
pub struct Report {
    pub projects: Vec<Project>,
    pub editor: Option<String>,
    pub ms: u64,
}

fn git(dir: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).creation_flags(NO_WINDOW).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

fn web_link(remote: &str) -> Option<String> {
    let r = remote.trim().trim_end_matches(".git");
    let r = if let Some(rest) = r.strip_prefix("git@") {
        format!("https://{}", rest.replacen(':', "/", 1))
    } else if let Some(rest) = r.strip_prefix("ssh://git@") {
        format!("https://{rest}")
    } else {
        r.to_string()
    };
    if !r.starts_with("https://") {
        return None;
    }
    // drop any user:token@ part
    Some(match r[8..].split_once('@') {
        Some((cred, rest)) if !cred.contains('/') => format!("https://{rest}"),
        _ => r,
    })
}

fn git_info(dir: &str) -> Option<Git> {
    if !Path::new(dir).join(".git").exists() {
        return None;
    }
    let mut g = Git::default();
    if let Some(s) = git(dir, &["status", "--porcelain=v2", "--branch"]) {
        for l in s.lines() {
            if let Some(v) = l.strip_prefix("# branch.head ") {
                g.branch = v.to_string();
            } else if let Some(v) = l.strip_prefix("# branch.upstream ") {
                g.upstream = Some(v.to_string());
            } else if let Some(v) = l.strip_prefix("# branch.ab ") {
                let mut it = v.split_whitespace();
                g.ahead = it.next().and_then(|x| x.trim_start_matches('+').parse().ok()).unwrap_or(0);
                g.behind = it.next().and_then(|x| x.trim_start_matches('-').parse().ok()).unwrap_or(0);
            } else if l.starts_with("? ") {
                g.untracked += 1;
            } else if l.starts_with("1 ") || l.starts_with("2 ") || l.starts_with("u ") {
                g.changed += 1;
            }
        }
    }
    if let Some(s) = git(dir, &["log", "-1", "--format=%ct%x09%s"]) {
        if let Some((t, m)) = s.trim().split_once('\t') {
            g.last_commit = t.parse().unwrap_or(0);
            g.last_message = m.chars().take(120).collect();
        }
    }
    g.remote = git(dir, &["remote", "get-url", "origin"]).and_then(|r| web_link(&r));
    Some(g)
}

/// What kind of project it is, from the files in its top folder.
fn tags(dir: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        names = rd.flatten().take(400).map(|e| e.file_name().to_string_lossy().to_lowercase()).collect();
    }
    let has = |n: &str| names.iter().any(|x| x == n);
    let ext = |e: &str| names.iter().any(|x| x.ends_with(e));
    let mut t: Vec<&str> = Vec::new();
    if has("src-tauri") { t.push("Tauri"); }
    if has("cargo.toml") { t.push("Rust"); }
    if has("package.json") {
        let pkg = std::fs::read_to_string(Path::new(dir).join("package.json")).unwrap_or_default();
        for (dep, name) in [("\"svelte\"", "Svelte"), ("\"react\"", "React"), ("\"vue\"", "Vue"), ("\"next\"", "Next.js"), ("\"electron\"", "Electron")] {
            if pkg.contains(dep) { t.push(name); }
        }
        t.push(if has("tsconfig.json") { "TypeScript" } else { "JavaScript" });
    }
    if has("pyproject.toml") || has("requirements.txt") || has("setup.py") || ext(".py") || ext(".ipynb") { t.push("Python"); }
    if has("go.mod") { t.push("Go"); }
    if ext(".sln") || ext(".csproj") { t.push("C#"); }
    if has("cmakelists.txt") || ext(".cpp") { t.push("C++"); }
    if ext(".uproject") { t.push("Unreal"); }
    if has("projectsettings") && has("assets") { t.push("Unity"); }
    if has("project.godot") { t.push("Godot"); }
    if has("pom.xml") || has("build.gradle") || has("build.gradle.kts") { t.push("Java"); }
    if has("dockerfile") || has("docker-compose.yml") || has("compose.yaml") { t.push("Docker"); }
    if t.is_empty() && (has("index.html") || ext(".html")) { t.push("Web"); }
    if t.is_empty() && (ext(".md") || ext(".docx") || ext(".pdf")) { t.push("Docs"); }
    t.dedup();
    t.into_iter().map(String::from).collect()
}

/// The first real sentence of the README, if there is one.
fn description(dir: &str) -> Option<String> {
    let rd = std::fs::read_dir(dir).ok()?;
    let readme = rd.flatten().map(|e| e.path()).find(|p| {
        let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        n == "readme.md" || n == "readme.txt" || n == "readme"
    })?;
    let text = std::fs::read_to_string(readme).ok()?;
    let mut in_code = false;
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || t.is_empty() || t.starts_with(['#', '!', '[', '<', '|', '-', '=', '>', '*']) {
            continue;
        }
        if t.split_once(". ").map_or(false, |(n, _)| n.chars().all(|ch| ch.is_ascii_digit())) {
            continue; // a numbered list
        }
        let clean: String = strip_links(&t.replace("**", "").replace('`', ""));
        let mut s: String = clean.chars().take(170).collect();
        if clean.chars().count() > 170 {
            s.push('…');
        }
        return Some(s);
    }
    None
}

/// "[Next.js](https://nextjs.org)" -> "Next.js"
fn strip_links(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('[') {
        let Some(j) = rest[i..].find("](") else { break };
        let Some(k) = rest[i + j..].find(')') else { break };
        out.push_str(&rest[..i]);
        out.push_str(&rest[i + 1..i + j]);
        rest = &rest[i + j + k + 1..];
    }
    out.push_str(rest);
    out
}

pub fn editor_path()-> Option<(String, PathBuf)> {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let cands = [
        ("VS Code", PathBuf::from(&local).join(r"Programs\Microsoft VS Code\Code.exe")),
        ("VS Code", PathBuf::from(r"C:\Program Files\Microsoft VS Code\Code.exe")),
        ("Cursor", PathBuf::from(&local).join(r"Programs\cursor\Cursor.exe")),
    ];
    cands.into_iter().find(|(_, p)| p.exists()).map(|(n, p)| (n.to_string(), p))
}

pub fn report(conn: &Connection) -> rusqlite::Result<Report> {
    let t0 = std::time::Instant::now();
    let g = graph::build(conn)?;
    let nodes: Vec<graph::Node> = g.nodes.into_iter().filter(|n| n.kind == "project" && n.path.is_some()).collect();
    let mut projects: Vec<Project> = nodes
        .into_par_iter()
        .map(|n| {
            let path = n.path.clone().unwrap_or_default();
            Project {
                name: n.label,
                size: n.size,
                newest: n.newest,
                scan_id: n.scan_id,
                dir_id: n.dir_id,
                backups: n.backups,
                tags: tags(&path),
                description: description(&path),
                git: git_info(&path),
                path,
            }
        })
        .collect();
    projects.sort_by(|a, b| b.newest.cmp(&a.newest));
    Ok(Report { projects, editor: editor_path().map(|e| e.0), ms: t0.elapsed().as_millis() as u64 })
}

pub fn open_in_editor(path: &str) -> Result<(), String> {
    let (_, exe) = editor_path().ok_or("No code editor found (VS Code or Cursor)")?;
    Command::new(exe).arg(path).creation_flags(NO_WINDOW).spawn().map(|_| ()).map_err(|e| e.to_string())
}

pub fn open_terminal(path: &str) -> Result<(), String> {
    if Command::new("wt.exe").args(["-d", path]).spawn().is_ok() {
        return Ok(());
    }
    Command::new("powershell.exe")
        .arg("-NoExit")
        .current_dir(path)
        .creation_flags(0x0000_0010) // a new console window
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}
