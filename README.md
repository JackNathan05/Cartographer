<p align="center"><img src="app-icon.png" width="96" alt="Cartographer icon"></p>

<h1 align="center">Cartographer</h1>
<p align="center"><i>A living map of your PC. See what's where, what's stale, and clean it up in one click.</i></p>

---

Cartographer is a lightweight Windows desktop app (Tauri + Rust + Svelte) that maps your drives,
folders, projects and apps — and, in later versions, suggests what to archive and drifts into a
cinematic 3D "show mode" when you step away. See [DESIGN.md](DESIGN.md) for the full plan.

## Status: v0.6 (project cards, timeline)

- **Fast scan:** reads the NTFS Master File Table directly (C: with 1.76M files in ~10 s) through a small elevated helper; falls back to a parallel folder walk
- **Map:** a force-directed graph of drives, folders, projects (git repos and `Projects` folders),
  games (Steam, Epic, Ubisoft, Xbox), installed apps and WSL distros, with search, filters and a details panel
- **Backups view:** where each backup goes (and whether it's up to date), a flow diagram of which folders feed which backup,
  how much of each drive is protected by 0 / 1 / 2+ backups, and which of your folders aren't backed up
  (games and reinstallable tools are labeled as such). Projects outside every backup show red on the map
- **Cleanup:** finds projects untouched for 6+ months, games not played in 6+ months (Steam + Windows launch history),
  apps not opened in a year, and old Downloads. One-click **Archive** (zip, verify every file, originals to the Recycle Bin),
  **Delete** (Recycle Bin), or **Uninstall…**; snooze or never-suggest; everything can be **Restored**. Archived items
  stay on the map as ghost dots
- **3D view:** the map in 3D (three.js, loaded only when opened): each folder level lifted to its own height, orbit / zoom, hover and click
- **Show mode:** after N minutes away, a fullscreen cinematic fly-around of the 3D map with captions and a clock. Any input
  closes it. Never starts during fullscreen apps / games, a locked screen, OBS, or on battery; stops after M minutes so
  Windows still sleeps; 30 fps cap. Preview button in the sidebar
- **Privacy mode:** real structure and sizes, every name replaced with random letters *then* blurred (the real text is
  never drawn). Can switch on automatically while OBS is open
- **Health:** one page of green / yellow / red checks: backup freshness and connection, free space per drive,
  Windows' own disk health (Get-PhysicalDisk), Recycle Bin / Temp / Desktop / old Downloads, unused items, how fresh
  each drive's scan is, and folders that aren't backed up. Each issue links to where you fix it; the tab shows a badge
- **Live updates:** every NTFS drive is watched for changes (noise like Temp, caches, Windows and the Recycle Bin is
  ignored); a changed drive is rescanned in the background at most every 30 min, only after a minute of no input
- **Projects:** a card for every project: what it's built with (Rust, Svelte, Python, Docker...), the first line of
  its README, size, last edit, git branch / uncommitted changes / unpushed commits / last commit, and which backups
  cover it. Open it in VS Code, Explorer, a terminal or on GitHub in one click. Filter to the ones that need a look
- **Timeline:** what changed today / this week / this month: folders that grew, shrank, appeared or were deleted
  (from a small daily snapshot kept after each scan), apps and games installed or updated, and big files added
  or changed. Drive size history as sparklines
- Local SQLite index
- Treemap "space view": click to zoom into folders, breadcrumbs, biggest-items list
- System / protected space shown as its own block, so the map adds up to what Windows reports
- Lives in the system tray; closing the window frees the UI, the tray keeps running
- Optional "Start with Windows" (starts hidden in the tray)

Everything stays on your PC. No telemetry.

## Build from source

Requirements: Windows 10/11, [Rust](https://rustup.rs), Node 20+, Visual Studio C++ build tools, WebView2 (built into Windows 11).

```powershell
npm install
npm run tauri dev      # run with hot reload
npm run tauri build    # installer in src-tauri/target/release/bundle
```

The index lives in `%LOCALAPPDATA%\app.cartographer.desktop\index.db`.

### Backup config

Tell the map where your backups go in `%LOCALAPPDATA%\app.cartographer.desktop\config.json`:

```json
{
  "backups": [
    { "name": "External drive (weekly)", "target": "D:\\Backup", "sources": ["C:\\Users\\me\\Documents", "E:\\Photos"] }
  ]
}
```

Anything inside a source counts as backed up; projects outside every backup show red on the map.

## Roadmap

| Version | Delivers |
| --- | --- |
| v0.1 | Scanner, index, space view, tray, fast MFT scan ✅ |
| v0.2 | 2D graph view: drives, projects, apps, games, backup links, search ✅ |
| v0.3 | Stale detection, suggestions, one-click archive / restore |
| v0.4 | 3D view, Show mode, Privacy mode |
| v0.5 | Health dashboard, live updates from the NTFS change journal |
| v0.6 | Project cards, Timeline |
| v0.7+ | Ask Claude, NAS / other machines, `.scr` screensaver, installer / open-source release |

## License

MIT
