<p align="center"><img src="app-icon.png" width="96" alt="Cartographer icon"></p>

<h1 align="center">Cartographer</h1>
<p align="center"><i>A living map of your PC. See what's where, what's stale, and clean it up in one click.</i></p>

---

Cartographer is a lightweight Windows desktop app (Tauri + Rust + Svelte) that maps your drives,
folders, projects and apps — and, in later versions, suggests what to archive and drifts into a
cinematic 3D "show mode" when you step away. See [DESIGN.md](DESIGN.md) for the full plan.

## Status: v0.1 (space view)

- Fast parallel scan of any drive into a local SQLite index
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

## Roadmap

| Version | Delivers |
| --- | --- |
| v0.1 | Scanner, index, space view, tray |
| v0.2 | 2D graph view: drives, projects, apps, games, backup links, search |
| v0.3 | Stale detection, suggestions, one-click archive / restore |
| v0.4 | 3D view, Show mode, Privacy mode |
| v0.5 | Health dashboard, live updates from the NTFS change journal |

## License

MIT
