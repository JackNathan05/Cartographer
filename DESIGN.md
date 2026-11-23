# Cartographer — Design Doc

*A living map of your PC. See what's where, what's stale, and clean it up in one click.*

Status: draft v0.1 · 2026-11-22 · Windows 10/11 · open source (planned)

---

## 1. What it is

Cartographer is a lightweight desktop app that turns your whole PC into an interactive map:
drives, folders, projects, apps, games, services, backups and cloud storage as **nodes**, and the
relationships between them as **links**. On top of the map sits a **cleanup engine** that spots
things you haven't used in a while and offers to archive or delete them — always reversible.

**The loop:** *Understand* (map) → *Suggest* (stale + space) → *Act* (one-click archive / restore).

When you step away, it drifts into **Show mode**: a slow, cinematic 3D fly-around of your PC's
graph — like the garage B-roll in a racing game.

### Goals
- **Fast:** full scan of a drive in seconds, UI that never stutters.
- **Light:** small install, low RAM, near-zero CPU/GPU when you're not looking at it.
- **Safe:** nothing permanent without a confirm; every action logged and undoable.
- **Private:** everything stays on the PC. No telemetry. AI features opt-in.
- **Showable:** looks great, has a Privacy mode for screenshots/streams, open-source ready.

### Non-goals (for now)
- Not an antivirus, registry cleaner, or "PC optimizer".
- Not a backup tool (it *shows* backup coverage; the existing backup scripts do the backing up).
- Not cross-platform at first (Windows only; keep the core portable where it's free to do so).

---

## 2. Core features

### 2.1 The map (graph view)
- **2D graph** for everyday use (fast, readable), **3D graph** for exploring and Show mode.
- Node size = disk size; colour = type (drive, project, app, game, media, service, backup).
- **Three depth levels** so it stays readable and fast:
  1. **Always drawn:** drives, top folders, projects, apps, games, services, backup targets (a few hundred nodes).
  2. **Indexed, not drawn:** every file is in the database (accurate sizes, last-used, duplicates).
  3. **On demand:** click a folder to expand its children.
- Search box (instant, from the index): type "airsim" → camera flies to it.

**Link types**
| Link | Example | Why it matters |
|---|---|---|
| lives on | Sea of Thieves → E: | where space goes |
| backed up to | Projects → Seagate, → Google Drive | anything with **no** backup link glows as a warning |
| depends on | NanoClaw → WSL → Ubuntu | don't delete something that breaks something else |
| duplicate of | same file in two places | easy space wins |
| related to | project code ↔ school folder ↔ Drive docs | context |

### 2.2 Space view
Treemap / sunburst of any drive or folder — "what's eating my drive". Click to zoom.

### 2.3 Stale detection + suggestions
"Last used" is assembled from several signals, because Windows doesn't track it reliably alone:
| Thing | Signals |
|---|---|
| Apps & games | UserAssist launch history, Prefetch, launcher data (Steam/Epic/Xbox) |
| Projects | last git commit, newest file modified |
| Files & folders | last modified, last access (where enabled) |
| Downloads & installers | age + type (installer already run, app already installed) |

Each suggestion explains itself and shows the payoff:
> **Archive AirSim?** Last touched 14 months ago · 7.3 GB · already backed up to Seagate
> [Archive] [Delete] [Snooze 30 days] [Never suggest]

A **Suggestions panel** sorts by "space freed" and "how sure we are". Rules are user-editable
(e.g. "never suggest anything in School", "games: stale after 6 months").

### 2.4 One-click archive / restore
1. Zip to `E:\Archive\<category>\<name>.zip` (configurable).
2. **Verify** the zip against the originals (count + size + hash).
3. Send originals to the **Recycle Bin** (not permanent delete).
4. Log it to the action log.
5. Leave an **"archived" ghost node** on the map → click **Restore** to unzip back in place.

Delete = Recycle Bin. Permanent delete only via an explicit confirm. Apps are uninstalled via
their own uninstaller, never by deleting folders.

### 2.5 Health dashboard
Backup age, free space per drive, drive health (SMART), Recycle Bin size, `_Import` waiting,
Desktop clutter. Green / yellow / red. Replaces the monthly upkeep report popup over time.

### 2.6 Bigger ideas (later versions)
- **Ask Claude:** "where's my AirSim project?", "what can I delete to free 50 GB?" (opt-in, sends
  only metadata the user approves).
- **Timeline:** what changed this week (new installs, big downloads, growth).
- **Project cards:** last edit, git status, size, backup status, open-in-editor.
- **More machines:** NAS and other PCs as extra nodes.
- **Right-click actions:** open, reveal, move, add to backup — all approval-first.

---

## 3. Show mode (idle B-roll)

After **N minutes** with no keyboard/mouse input (default 10), Cartographer fades into a fullscreen
3D view of the graph and slowly orbits it: gentle camera paths, depth-of-field, nodes pulsing softly,
occasional "fly-bys" to the biggest or most recently changed nodes with a subtle caption
("Projects · 24.2 GB · 9,250 files").

**Behaviour rules (so it never gets in the way):**
- **Exits instantly** on any input. Never steals keystrokes (the first key/mouse just wakes it).
- **Does not start** when: a fullscreen app or game is running, OBS is recording/streaming,
  a video call is active, or the PC is on battery.
- **Respects sleep:** runs for a set time (default 20 min) then lets the monitor turn off and the
  PC sleep as normal — it never blocks Windows power settings.
- **GPU budget:** capped at 30 fps, reduced node count, low-cost shaders. Target: barely
  noticeable in Task Manager.
- **Multi-monitor:** choose which screen(s); others stay as they are.
- Optional later: install as a real Windows screensaver (`.scr`) instead of the app's own timer.

---

## 4. Always-on (start with Windows, live in the tray)

**Decision: yes, with guardrails.** It starts at logon, lives in the system tray, and "closing"
the window just hides it. A global hotkey (e.g. `Win+Shift+M`) brings it back instantly.

Guardrails that keep this from becoming another startup hog:
- **Two parts:** a tiny Rust **core** (tray, file-change watcher, idle detection, scheduler — target
  < 20 MB RAM, ~0% CPU) and the **UI window**, which is *destroyed* when hidden and re-created on
  open (UI loads from a warm cache in well under a second).
- **Delayed start:** waits ~60 s after logon so it doesn't slow boot.
- Heavy work (full rescans, hashing for duplicates) runs at **low priority** and only when idle.
- "Pause Cartographer" and "Quit" in the tray menu. Startup can be turned off in Settings.

**Admin access:** the fastest scan (reading the NTFS master file table, like WizTree/Everything)
and the change journal need admin rights. Plan: a small helper registered once as a scheduled task
"run with highest privileges" (no UAC prompt each boot). Without it, Cartographer falls back to a
normal (slower) scan. The UI itself never runs as admin.

---

## 5. Privacy mode ("demo mode")

Shows the **real** PC — real structure, sizes, colours, links — but **hides every name**.

- File and folder names, paths, app names, search results, tooltips, window title, captions in
  Show mode: all hidden.
- **How it's hidden matters:** real text under a CSS blur can sometimes be guessed from a
  screenshot. So the UI **replaces** names with random glyphs of similar length *before* blurring.
  Looks blurred; the real name was never drawn on screen.
- Toggle from the tray / hotkey; optional "auto-privacy when OBS is running or screen sharing".
- Optional allow-list: show drive letters and top-level categories (e.g. "Games", "Projects"),
  hide everything below.

---

## 6. Architecture

```
┌──────────────────────── Cartographer.exe (Tauri) ────────────────────────┐
│  Rust core (always running)                                               │
│   • Scanner: MFT reader (admin helper) / fast fallback walk               │
│   • Watcher: NTFS USN change journal → incremental updates                │
│   • Signals: UserAssist, Prefetch, git, launchers, backup logs, SMART     │
│   • Rules engine → Suggestions                                            │
│   • Actions: archive / verify / recycle / restore + action log            │
│   • Idle detector (GetLastInputInfo) + fullscreen/OBS checks → Show mode  │
│   • Tray, hotkey, scheduler                                               │
│                 │  SQLite (index.db, WAL)                                 │
│                 ▼                                                         │
│  UI (WebView2, created on demand)                                         │
│   • Svelte or Solid (tiny, fast) + TypeScript                             │
│   • 2D graph: Sigma.js (WebGL)   • 3D / Show mode: three.js               │
│   • Treemap: d3-hierarchy on canvas                                        │
└───────────────────────────────────────────────────────────────────────────┘
```

**Why Tauri:** ~5–10 MB installer and far lower RAM than Electron (uses Windows' built-in WebView2).
Rust is a good fit for the scanner and file actions (speed + safety).

**Data model (SQLite, simplified)**
- `nodes(id, kind, name, path, parent_id, size, file_count, modified, last_used, last_used_source, backed_up)`
- `links(src, dst, kind)`
- `files(id, node_id, name, size, modified, hash?)` — full index, not drawn
- `suggestions(id, node_id, action, reason, bytes, confidence, state)`
- `actions(id, time, kind, from, to, verified, undo_info)` — the action log
- `settings` — paths, rules, privacy, show-mode options (no personal paths hard-coded in the code)

**Performance targets**
| Metric | Target |
|---|---|
| Installer | < 15 MB |
| Core idle RAM (UI closed) | < 20 MB |
| UI open RAM | < 200 MB |
| Full scan, 500 GB NTFS (admin) | < 15 s |
| Open UI from tray | < 1 s |
| Graph | 60 fps at 2,000 nodes (2D) |
| Show mode | ≤ 30 fps, low GPU |

---

## 7. Safety rules (non-negotiable)
1. No permanent deletes without an explicit confirm naming the item and size.
2. Archive = zip → verify → Recycle Bin → log → ghost node. Restore always available.
3. Never touch system folders, app install folders (use uninstallers), or anything a "depends on"
   link says is in use.
4. Protected paths list (default: Windows, Program Files, `.ssh`, credential files, WSL distros).
5. Every action in the action log, exportable.

---

## 8. Roadmap
| Version | Delivers |
|---|---|
| **v0.1** | Rust core + scanner + SQLite index + **Space view** (treemap). Tray + start with Windows. |
| **v0.2** | **Graph view** (2D): drives, projects, apps, games, services, backup links. Search. |
| **v0.3** | **Stale detection + Suggestions + one-click Archive / Restore** + action log. |
| **v0.4** | **3D view + Show mode** + **Privacy mode**. |
| **v0.5** | Health dashboard (replaces monthly report). Change-journal live updates. |
| **v0.6+** | Ask Claude, Timeline, Project cards, NAS / other machines, `.scr` screensaver, open-source release. |

*(Show mode could move earlier if we want something to show off sooner.)*

### v0.1 status (2026-11-22)
| Item | State |
|---|---|
| Rust core, SQLite index, parallel folder scan | Done (6.2 MB exe) |
| Space view: treemap, zoom, breadcrumbs, biggest-items list, reveal in Explorer | Done |
| "System & protected files" block so totals match Windows | Done |
| Tray icon, close-to-tray (UI destroyed, core keeps running), single instance | Done |
| Start with Windows (hidden in tray), opt-in toggle | Done |
| Full scan speed | **Done (fast scan):** MFT reader via elevated helper. C: 9.6 s scan / 16 s incl. save (was 46 s), E: 1.4 s. 0 unreadable folders (was 218). Folder walk stays as fallback |
| Full file index | **Partial:** files ≥ 1 MB stored individually, smaller files summed per folder (C: index = 76 MB) |
| Admin helper | **Done:** one UAC prompt copies the exe to Program Files (admin-only) + task `\Cartographer\Fast Scan` (highest privileges); app triggers it with `schtasks /Run`, talks via request/status JSON |
| USN change journal (live updates) | Not started → v0.5 |
| Global hotkey, 60 s delayed start | Not started |
| Hard-link double counting (WinSxS) | **Fixed by fast scan** (C:\Windows 36.0 → 31.6 GB); walk scan still over-counts |

---

## 9. Open source plan
- License: MIT or Apache-2.0 (decide before first public push).
- Repo hygiene from day one: no personal paths/names in code or tests; sample data is synthetic.
- Privacy mode doubles as the way to make screenshots/videos for the README.
- Signed releases later (unsigned Windows apps trigger SmartScreen warnings).

## 10. Open questions
- Svelte vs Solid for the UI (both tiny; pick one at v0.1).
- Idle timeout and show-mode length defaults (10 min / 20 min?).
- Archive location default: `E:\Archive` vs per-drive?
- How far "depends on" detection goes in v0.2 (start with services/WSL/venvs, expand later).
- Integrate with the existing Weekly-Backup / Monthly-Upkeep scripts, or replace them over time?

### v0.2 status (2026-11-22)
| Item | State |
|---|---|
| Graph builder (Rust): drives, top folders, projects, games, apps, WSL, backup targets | Done: 136 nodes on the dev PC, built in ~1–2 s |
| Project detection | Git repos + children of `Projects` folders in the user profile; category folders expanded; junk (node_modules, venv, build…) skipped |
| Game detection | Steam (libraryfolders.vdf + appmanifests), Epic manifests, Ubisoft registry, `XboxGames` folders, plus game-ish uninstall entries |
| Apps | Uninstall registry (HKLM 64/32 + HKCU) with an install location, ≥ 20 MB |
| Backup links + coverage | Config file `config.json` (`backups: [{name, target, sources}]`); not-backed-up projects drawn red |
| 2D graph UI (Sigma.js + ForceAtlas2) | Done: search + jump, kind filters, hover/selection focus, details panel, "Open in Space view" |
| Single-child folder chains collapsed | Done |
| Settings UI for backups | Not started (config file for now) |
| "Depends on" links (services, venvs) | Not started |

### Map layout + Backups view (2026-11-22)
| Item | State |
|---|---|
| Live, editable map layout (d3-force) | Done: drive-vs-drive repulsion, radiate/pull/push points, drag to pin, force sliders, saved to `layout.json` |
| Backup nodes/lines removed from the map | Done: the map only colors coverage (red = not backed up) |
| Backups view | Done: destination cards (connected, last activity, stored, free space), source → destination flow diagram, per-drive coverage bars (2+ / 1 / yours unprotected / reinstallable), not-backed-up vs backed-up folder lists |
| Backup editor UI (instead of hand-editing config.json) | Not started |
| Real "last run" from backup logs / tool integrations | Heuristic only (newest timestamp in target + nearby logs) |

### v0.3 status (2026-11-23)
| Item | State |
|---|---|
| Stale detection | Projects (newest file / git activity, 180 d), games (Steam LastPlayed + UserAssist, 180 d, >= 1 GB), apps (UserAssist, 365 d, >= 200 MB; unknown history never suggested), Downloads (60 d, >= 10 MB) |
| Suggestions | Reason, last used + source, size, backup coverage; Snooze 30 days / Never suggest (cleanup.json) |
| Archive | Zip (deflate, zip64) to archive dir (config `archive_dir`, else `<data drive>:\Archive`, else profile\Archive) -> verify count, sizes, CRC -> originals to Recycle Bin -> log. Junctions skipped. Any failure before the Recycle Bin step deletes only the partial zip |
| Delete / Uninstall | Recycle Bin (restorable) / opens Windows Installed apps |
| Restore | Unzip in place + verify count, zip to Recycle Bin; deletes restored from the Recycle Bin |
| Safety | Refuses drive roots, Windows / Program Files / ProgramData / AppData / dot-folders, whole user folders, anything on C: outside the profile |
| Ghost nodes | Archived items appear on the map (faded) with a Restore button |
| Self-test | `--cleanup-selftest` archives + restores and deletes + restores a throwaway folder: 12/12 pass |
| Dot size controls | Dot size (%) and Shrink per level (%) in the layout editor |
