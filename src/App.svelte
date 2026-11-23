<script>
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { enable, disable, isEnabled } from '@tauri-apps/plugin-autostart';
  import { onMount } from 'svelte';
  import Treemap from './Treemap.svelte';
  import GraphView from './GraphView.svelte';
  import BackupView from './BackupView.svelte';
  import CleanupView from './CleanupView.svelte';
  import { fmtBytes, fmtCount, fmtAgo, fmtDate } from './format.js';

  let drives = $state([]);
  let scans = $state([]);
  let view = $state(null);
  let progress = $state(null);
  let hover = $state(null);
  let autostart = $state(false);
  let fast = $state(false);
  let fastBusy = $state(false);
  let error = $state('');
  let tab = $state('map');
  let graphData = $state(null);
  let graphLoading = $state(false);
  let backupData = $state(null);

  const scanFor = (letter) => scans.find((s) => s.root.toUpperCase().startsWith(letter + ':'));
  const kids = $derived(view?.node?.children ?? []);
  const total = $derived(view?.node?.size ?? 0);

  async function refresh() {
    drives = await invoke('drives');
    scans = await invoke('scans');
  }

  async function openScan(scan, dirId = null) {
    const v = await invoke('view', { scanId: scan.id, dirId });
    if (v) view = v;
  }

  async function openDir(id) {
    if (!view) return;
    const v = await invoke('view', { scanId: view.scan_id, dirId: id });
    if (v) { view = v; hover = null; }
  }

  function up() {
    if (view && view.crumbs.length > 1) openDir(view.crumbs[view.crumbs.length - 2].id);
  }

  async function scan(letter) {
    error = '';
    try {
      progress = { root: letter + ':\\', files: 0, dirs: 0, bytes: 0, seconds: 0 };
      await invoke('start_scan', { root: letter + ':\\' });
    } catch (e) {
      progress = null;
      error = String(e);
    }
  }

  async function toggleAutostart() {
    try {
      if (autostart) await disable(); else await enable();
      autostart = await isEnabled();
    } catch (e) { error = String(e); }
  }

  async function toggleFast() {
    fastBusy = true;
    error = '';
    try { fast = await invoke('set_fast_scan', { on: !fast }); }
    catch (e) { error = String(e); }
    fastBusy = false;
  }

  function phaseLabel(p) {
    if (p.phase === 'reading index' && p.total_records && p.records >= p.total_records) return 'building map';
    return p.phase;
  }

  async function loadGraph() {
    graphLoading = true;
    try { graphData = await invoke('graph'); }
    catch (e) { error = String(e); }
    graphLoading = false;
  }

  async function showBackups() {
    tab = 'backups';
    try { backupData = await invoke('backup_report'); }
    catch (e) { error = String(e); }
  }

  function showMap() {
    tab = 'map';
    if (!graphData && !graphLoading && scans.length) { loadGraph(); return; }
    if (!graphData && !graphLoading && scans.length) loadGraph();
  }

  async function openSpace(scanId, dirId) {
    const v = await invoke('view', { scanId, dirId });
    if (v) { view = v; hover = null; tab = 'space'; }
  }

  function openDrive(s) {
    tab = 'space';
    openScan(s);
  }

  function onKey(e) {
    if (tab !== 'space' || ['INPUT', 'TEXTAREA'].includes(e.target?.tagName)) return;
    if (e.key === 'Backspace' || e.key === 'Escape') up();
  }

  onMount(() => {
    const un = [];
    (async () => {
      await refresh();
      try { autostart = await isEnabled(); } catch {}
      fast = await invoke('helper_status');
      const c = scanFor('C') ?? scans[0];
      if (c) openScan(c);
      if (scans.length) loadGraph(); else tab = 'space';
      if (await invoke('is_scanning')) progress = { root: '', files: 0, dirs: 0, bytes: 0, seconds: 0 };
      un.push(await listen('scan-progress', (e) => (progress = e.payload)));
      un.push(await listen('scan-done', async (e) => {
        progress = null;
        await refresh();
        const s = scans.find((x) => x.id === e.payload.scan_id);
        if (s) openScan(s);
        loadGraph();
        if (tab === 'backups') showBackups();
      }));
      un.push(await listen('scan-error', (e) => { progress = null; error = String(e.payload); }));
    })();
    return () => un.forEach((f) => f());
  });

  const info = $derived(hover ?? view?.node);
</script>

<svelte:window onkeydown={onKey} />

<div class="app">
  <aside>
    <div class="brand">
      <img src="/icon.png" alt="" />
      <div>
        <h1>Cartographer</h1>
        <p>v0.3 · map, space, backups &amp; cleanup</p>
      </div>
    </div>

    <h2>Drives</h2>
    {#each drives as d (d.letter)}
      {@const s = scanFor(d.letter)}
      {@const used = d.total - d.free}
      {@const pct = d.total ? used / d.total : 0}
      {@const active = tab === 'space' && view && s && view.scan_id === s.id}
      <div class="drive" class:active>
        <button class="dmain" disabled={!s} onclick={() => s && openDrive(s)}>
          <div class="drow">
            <span class="letter">{d.letter}:</span>
            <span class="label">{d.label || (d.kind === 'removable' ? 'USB drive' : 'Local disk')}</span>
            <span class="free">{fmtBytes(d.free)} free</span>
          </div>
          <div class="bar"><div style="width:{pct * 100}%" class:warn={pct > 0.9}></div></div>
          <div class="meta">
            {#if s}
              {fmtBytes(used)} of {fmtBytes(d.total)} · scanned {fmtAgo(s.started)} in {(s.duration_ms / 1000).toFixed(1)} s{s.method === 'fast' ? ' ⚡' : ''}
            {:else}
              {fmtBytes(used)} of {fmtBytes(d.total)} · not scanned yet
            {/if}
          </div>
        </button>
        <button class="scanbtn" disabled={!!progress} onclick={() => scan(d.letter)} title="Scan {d.letter}:">
          {s ? '↻' : 'Scan'}
        </button>
      </div>
    {/each}

    {#if progress}
      <div class="progress">
        <div class="spinner"></div>
        <div>
          <b>Scanning {progress.root}{phaseLabel(progress) ? ` · ${phaseLabel(progress)}` : ''}</b>
          {#if progress.total_records && phaseLabel(progress) === 'reading index'}
            <span>{Math.round((progress.records / progress.total_records) * 100)}% of {fmtCount(progress.total_records)} records · {progress.seconds?.toFixed(1)} s</span>
          {:else}
            <span>{fmtCount(progress.files)} files · {fmtBytes(progress.bytes)} · {progress.seconds?.toFixed(1)} s</span>
          {/if}
        </div>
      </div>
    {/if}
    {#if error}<div class="error">{error}</div>{/if}

    <div class="spacer"></div>
    <div class="fast" class:on={fast}>
      <div>
        <b>⚡ Fast scan {fast ? 'on' : 'off'}</b>
        <span>{fast ? 'Reads the drive index directly. NTFS drives scan in seconds.' : 'Reads the drive index directly. Needs admin once (one Windows prompt).'}</span>
      </div>
      <button disabled={fastBusy || !!progress} onclick={toggleFast}>{fastBusy ? '…' : fast ? 'Turn off' : 'Turn on'}</button>
    </div>
    <label class="toggle">
      <input type="checkbox" checked={autostart} onchange={toggleAutostart} />
      <span>Start with Windows (in tray)</span>
    </label>
    <p class="hint">Closing the window keeps Cartographer in the tray. Quit from the tray icon.</p>
  </aside>

  <main>
    <header>
      <div class="tabs">
        <button class:on={tab === 'map'} onclick={showMap}>Map</button>
        <button class:on={tab === 'space'} onclick={() => (tab = 'space')}>Space</button>
        <button class:on={tab === 'backups'} onclick={showBackups}>Backups</button>
        <button class:on={tab === 'cleanup'} onclick={() => (tab = 'cleanup')}>Cleanup</button>
      </div>
      {#if tab === 'space' && view}
        <nav>
          {#each view.crumbs as c, i (c.id)}
            {#if i > 0}<span class="sep">›</span>{/if}
            <button class:cur={i === view.crumbs.length - 1} onclick={() => openDir(c.id)}>{c.name}</button>
          {/each}
        </nav>
        <div class="tot">{fmtBytes(total)} · {fmtCount(view.node.files)} files</div>
      {:else}
        <nav></nav>
        {#if tab === 'map' && graphData}
          <button class="rebuild" disabled={graphLoading} onclick={loadGraph}>{graphLoading ? 'Updating…' : '↻ Refresh map'}</button>
        {/if}
      {/if}
    </header>
    {#if tab === 'cleanup'}
      <div class="body single">
        <CleanupView onspace={openSpace} onreveal={(p) => invoke('reveal', { path: p })} onchanged={() => { graphData = null; }} />
      </div>
    {:else if tab === 'backups'}
      {#if backupData}
        <div class="body single">
          <BackupView report={backupData} onspace={openSpace} onreveal={(p) => invoke('reveal', { path: p })} />
        </div>
      {:else}
        <div class="empty"><h2>Reading your backups…</h2></div>
      {/if}
    {:else if tab === 'map'}
      {#if graphData}
        <div class="body">
          {#key graphData}
            <GraphView data={graphData} onspace={openSpace} onreveal={(p) => invoke('reveal', { path: p })} onchanged={loadGraph} />
          {/key}
        </div>
      {:else}
        <div class="empty">
          <img src="/icon.png" alt="" />
          {#if graphLoading}
            <h2>Drawing your map…</h2>
            <p>Reading drives, projects, games and apps.</p>
          {:else}
            <h2>Map your PC</h2>
            <p>Scan a drive on the left first. The map fills in as you scan more drives.</p>
          {/if}
        </div>
      {/if}
    {:else if view}
      <div class="body">
        <section class="map">
          {#key view.node.id}
            <Treemap node={view.node} onopen={openDir} onhover={(d) => (hover = d)} />
          {/key}
        </section>
        <section class="list">
          <h3>Biggest here</h3>
          <ul>
            {#each kids as k (k.kind + (k.id ?? k.path ?? k.name))}
              <li class:clickable={k.kind === 'dir'}>
                <button
                  disabled={k.kind !== 'dir'}
                  onclick={() => k.id && openDir(k.id)}
                  onmouseenter={() => (hover = k)}
                  onmouseleave={() => (hover = null)}
                >
                  <span class="ico {k.kind}"></span>
                  <span class="nm">{k.name}</span>
                  <span class="sz">{fmtBytes(k.size)}</span>
                  <span class="pbar"><span style="width:{total ? (k.size / total) * 100 : 0}%"></span></span>
                </button>
                {#if k.path}
                  <button class="rv" title="Show in Explorer" onclick={() => invoke('reveal', { path: k.path })}>↗</button>
                {/if}
              </li>
            {/each}
          </ul>
        </section>
      </div>
      <footer>
        {#if info}
          <b>{info.name}</b>
          <span>{fmtBytes(info.size)}</span>
          {#if info.kind === 'dir'}<span>{fmtCount(info.files)} files</span>{/if}
          {#if info.newest}<span>last changed {fmtDate(info.newest)} ({fmtAgo(info.newest)})</span>{/if}
          {#if info.kind === 'hidden'}<span>Windows, page file, restore points and space this scan can't account for</span>{/if}
          {#if info.kind === 'other'}<span>small files and folders grouped together</span>{/if}
          {#if info.denied && info.kind === 'dir'}<span class="warn">access denied</span>{/if}
          {#if info.path}<span class="path">{info.path}</span>{/if}
        {/if}
      </footer>
    {:else}
      <div class="empty">
        <img src="/icon.png" alt="" />
        <h2>Map your PC</h2>
        <p>Pick a drive on the left and press <b>Scan</b>.</p>
      </div>
    {/if}
  </main>
</div>
