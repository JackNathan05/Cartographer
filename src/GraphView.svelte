<script>
  import Graph from 'graphology';
  import Sigma from 'sigma';
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import { fmtBytes, fmtAgo, fmtDate } from './format.js';
  import { createLayout, defaultLayout, DEFAULTS, SLIDERS, POINT_TYPES } from './layout.js';

  let { data, onspace, onreveal } = $props();

  const KINDS = {
    drive: { label: 'Drives', color: '#e2e8f0' },
    folder: { label: 'Folders', color: '#3b4a61' },
    project: { label: 'Projects', color: '#38bdf8' },
    game: { label: 'Games', color: '#4ade80' },
    app: { label: 'Apps', color: '#a78bfa' },
    service: { label: 'Services', color: '#fb923c' },
  };
  const WARN = '#f87171';
  const DIM = '#1a2333';

  let container;
  let renderer = null;
  let graph = null;
  let engine = null;
  let hovered = $state(null);
  let selected = $state(null);
  let query = $state('');
  let hidden = $state({});
  let counts = $state({});

  // layout editing
  let layout = $state(defaultLayout());
  let editing = $state(false);
  let placing = $state(null); // point type being placed, or null
  let selectedPoint = $state(null);
  let dirty = $state(false);
  let saving = $state(false);
  let savedNote = $state('');
  let pinnedCount = $state(0);

  const byId = $derived(new Map(data.nodes.map((n) => [n.id, n])));
  const sel = $derived(selected && !selected.startsWith('pt:') ? byId.get(selected) : null);

  function nodeSize(n) {
    if (n.kind === 'drive') return 18;
    const gb = Math.max(n.size, 0) / 1073741824;
    const s = Math.min(22, 3 + Math.sqrt(gb) * 2.2);
    return n.kind === 'folder' ? Math.max(3, s * 0.6) : s;
  }

  function colorOf(n) {
    if (n.kind === 'project' && n.backed_up === false) return WARN;
    return KINDS[n.kind]?.color ?? '#94a3b8';
  }

  function buildGraph() {
    const g = new Graph({ multi: false, type: 'directed' });
    const c = {};
    for (const n of data.nodes) {
      c[n.kind] = (c[n.kind] ?? 0) + 1;
      g.addNode(n.id, {
        label: n.label, size: nodeSize(n), color: colorOf(n), kind: n.kind,
        forceLabel: n.kind === 'drive' || n.kind === 'backup', x: 0, y: 0,
      });
    }
    counts = c;
    for (const l of data.links) {
      if (!g.hasNode(l.source) || !g.hasNode(l.target) || g.hasEdge(l.source, l.target)) continue;
      const backup = l.kind === 'backed_up_to';
      g.addEdge(l.source, l.target, {
        kind: l.kind, color: backup ? 'rgba(45,212,191,0.55)' : '#243045', size: backup ? 1.4 : 1,
      });
    }
    for (const p of layout.points) addPointNode(g, p);
    return g;
  }

  function addPointNode(g, p) {
    g.addNode(`pt:${p.id}`, {
      label: POINT_TYPES[p.type].label, size: 10, color: POINT_TYPES[p.type].color, kind: 'point',
      forceLabel: true, x: p.x, y: p.y, zIndex: 3,
    });
  }

  // ---- simulation -> graph
  let frame = 0;
  function onTick() {
    if (!engine || !graph) return;
    for (const n of engine.nodes) {
      graph.setNodeAttribute(n.id, 'x', n.x);
      graph.setNodeAttribute(n.id, 'y', n.y);
    }
    if (!frame) frame = requestAnimationFrame(() => { frame = 0; renderer?.refresh(); });
  }

  function fitBBox() {
    let m = 200;
    for (const n of engine.nodes) m = Math.max(m, Math.abs(n.x), Math.abs(n.y));
    for (const p of layout.points) m = Math.max(m, Math.abs(p.x), Math.abs(p.y));
    m *= 1.15;
    renderer.setCustomBBox({ x: [-m, m], y: [-m, m] });
  }

  function start(saved) {
    const base = defaultLayout();
    layout = {
      ...base,
      ...(saved ?? {}),
      settings: { ...DEFAULTS, ...(saved?.settings ?? {}) },
      points: saved?.points ?? base.points,
      pinned: saved?.pinned ?? {},
      positions: saved?.positions ?? {},
    };
    pinnedCount = Object.keys(layout.pinned).length;
    graph = buildGraph();
    engine = createLayout(data, layout, onTick);
    if (!Object.keys(layout.positions).length) {
      // settle most of the way before the first frame so the map doesn't explode on screen
      engine.sim.stop();
      engine.sim.tick(220);
      engine.sim.alpha(0.2).restart();
    }
    onTick();
    renderer = makeRenderer();
    fitBBox();
    renderer.refresh();
  }

  function stopAll() {
    engine?.stop();
    renderer?.kill();
    engine = null;
    renderer = null;
  }

  function markDirty() {
    dirty = true;
    savedNote = '';
  }

  // ---- labels: every label gets a dark backing so it reads over lines and dots, and labels
  // never sit on top of each other: each one tries right / left / above / below its dot and is
  // skipped if all four are taken (bigger dots are drawn first, so they win). Labels that must
  // show (drives, hovered, selected, search hits) are drawn even when crowded.
  const occupied = [];
  const hits = (a) => occupied.some((b) => a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y);
  function drawLabel(ctx, d, s) {
    if (!d.label) return;
    const size = s.labelSize;
    ctx.font = `${s.labelWeight} ${size}px ${s.labelFont}`;
    const w = ctx.measureText(d.label).width + 10;
    const h = size + 7;
    const gap = d.size + 4;
    const spots = [
      { x: d.x + gap, y: d.y - h / 2 },
      { x: d.x - gap - w, y: d.y - h / 2 },
      { x: d.x - w / 2, y: d.y - gap - h },
      { x: d.x - w / 2, y: d.y + gap },
    ];
    const dpr = window.devicePixelRatio || 1;
    const cw = ctx.canvas.width / dpr, ch = ctx.canvas.height / dpr;
    const inside = (p) => p.x >= 2 && p.y >= 2 && p.x + w <= cw - 2 && p.y + h <= ch - 2;
    let spot = spots.find((p) => inside(p) && !hits({ ...p, w, h }));
    if (!spot) {
      if (!d.forceLabel) return;
      spot = spots[0];
    }
    occupied.push({ ...spot, w, h });
    ctx.fillStyle = 'rgba(9,13,21,0.82)';
    ctx.beginPath();
    ctx.roundRect(spot.x, spot.y, w, h, 5);
    ctx.fill();
    ctx.fillStyle = s.labelColor.color;
    ctx.fillText(d.label, spot.x + 5, spot.y + h / 2 + size / 3);
  }

  // ---- interaction helpers
  function drawHover(ctx, d, s) {
    const size = s.labelSize;
    ctx.font = `600 ${size}px ${s.labelFont}`;
    const label = d.label ?? '';
    const w = ctx.measureText(label).width + 16;
    const h = size + 12;
    const x = d.x + d.size + 6;
    const y = d.y - h / 2;
    ctx.fillStyle = 'rgba(10,15,24,0.92)';
    ctx.strokeStyle = 'rgba(94,234,212,0.6)';
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.roundRect(x, y, w, h, 6);
    ctx.fill();
    ctx.stroke();
    ctx.fillStyle = '#e6edf7';
    ctx.fillText(label, x + 8, y + h / 2 + size / 3);
    ctx.beginPath();
    ctx.arc(d.x, d.y, d.size + 3, 0, Math.PI * 2);
    ctx.strokeStyle = 'rgba(94,234,212,0.9)';
    ctx.lineWidth = 2;
    ctx.stroke();
  }

  function focusSet() {
    const f = hovered ?? selected;
    if (!f || !graph?.hasNode(f) || f.startsWith('pt:')) return null;
    const s = new Set([f]);
    graph.forEachNeighbor(f, (n) => s.add(n));
    return s;
  }

  function matches(id) {
    const q = query.trim().toLowerCase();
    if (!q) return true;
    const n = byId.get(id);
    if (!n) return false;
    return n.label.toLowerCase().includes(q) || (n.path ?? '').toLowerCase().includes(q);
  }

  $effect(() => {
    hovered; selected; query; hidden; editing; selectedPoint; pinnedCount;
    renderer?.refresh({ skipIndexation: true });
  });

  function flyTo(id) {
    if (!renderer || !graph.hasNode(id)) return;
    const p = renderer.getNodeDisplayData(id);
    if (p) renderer.getCamera().animate({ x: p.x, y: p.y, ratio: 0.35 }, { duration: 650 });
  }

  function onSearchKey(e) {
    if (e.key === 'Enter') {
      const hit = data.nodes.find((n) => !hidden[n.kind] && matches(n.id));
      if (hit) { selected = hit.id; flyTo(hit.id); }
    }
    if (e.key === 'Escape') query = '';
  }

  function resetView() {
    selected = null;
    if (renderer && engine) fitBBox();
    renderer?.getCamera().animate({ x: 0.5, y: 0.5, ratio: 1, angle: 0 }, { duration: 500 });
  }

  const neighbors = $derived.by(() => {
    pinnedCount;
    if (!sel || !graph?.hasNode(sel.id)) return { up: [], down: [], backups: [] };
    const up = [], down = [], backups = [];
    graph.forEachOutEdge(sel.id, (e, a, s, t) => (a.kind === 'backed_up_to' ? backups : up).push(byId.get(t)));
    graph.forEachInEdge(sel.id, (e, a, s) => down.push(byId.get(s)));
    down.sort((a, b) => b.size - a.size);
    return { up, down: down.slice(0, 12), more: Math.max(0, down.length - 12), backups };
  });

  // ---- layout editor actions
  function changed(heat = 0.5) {
    engine?.update(heat);
    markDirty();
  }

  function addPoint(type, x, y) {
    const p = { id: Math.random().toString(36).slice(2, 8), type, x, y, strength: 1, reach: type === 'radiate' ? 0 : 300 };
    layout.points.push(p);
    addPointNode(graph, p);
    selectedPoint = p.id;
    changed(0.7);
  }

  function removePoint(id) {
    layout.points = layout.points.filter((p) => p.id !== id);
    if (graph.hasNode(`pt:${id}`)) graph.dropNode(`pt:${id}`);
    if (selectedPoint === id) selectedPoint = null;
    changed(0.7);
  }

  function setPointType(p, type) {
    p.type = type;
    graph.mergeNodeAttributes(`pt:${p.id}`, { label: POINT_TYPES[type].label, color: POINT_TYPES[type].color });
    if (type !== 'radiate' && !p.reach) p.reach = 300;
    changed(0.6);
  }

  function unpinSelected() {
    engine.unpin(selected);
    pinnedCount = Object.keys(layout.pinned).length;
    markDirty();
  }

  function unpinAll() {
    engine.unpinAll();
    pinnedCount = 0;
    markDirty();
  }

  async function save() {
    saving = true;
    try {
      const snap = JSON.parse(JSON.stringify(engine.snapshot()));
      await invoke('save_layout', { layout: snap });
      dirty = false;
      savedNote = 'Saved. The map will always open like this.';
    } catch (e) {
      savedNote = 'Save failed: ' + e;
    }
    saving = false;
  }

  async function revert() {
    const saved = await invoke('load_layout');
    stopAll();
    selectedPoint = null;
    start(saved);
    dirty = false;
    savedNote = saved ? 'Back to your last save.' : 'Nothing saved yet, back to default.';
  }

  async function resetDefault() {
    stopAll();
    selectedPoint = null;
    start(null);
    markDirty();
    savedNote = 'Default layout. Save to keep it.';
  }

  function pointAtPin(id) {
    return layout.points.find((p) => `pt:${p.id}` === id);
  }

  // ---- renderer + dragging
  function makeRenderer() {
    const r = new Sigma(graph, container, {
      labelColor: { color: '#cbd5e1' },
      labelFont: "'Segoe UI Variable Text', 'Segoe UI', sans-serif",
      labelSize: 12,
      labelWeight: '500',
      labelDensity: 1,
      labelGridCellSize: 60,
      labelRenderedSizeThreshold: 6,
      defaultDrawNodeLabel: drawLabel,
      defaultDrawNodeHover: drawHover,
      zIndex: true,
      stagePadding: 40,
      minCameraRatio: 0.05,
      maxCameraRatio: 4,
      nodeReducer: (id, a) => {
        const res = { ...a };
        if (a.kind === 'point') {
          if (!editing) { res.hidden = true; return res; }
          res.zIndex = 3;
          if (id === `pt:${selectedPoint}`) { res.highlighted = true; res.size = 13; }
          return res;
        }
        if (hidden[a.kind]) { res.hidden = true; return res; }
        const f = focusSet();
        const q = query.trim();
        if ((f && !f.has(id)) || (q && !matches(id))) {
          res.color = DIM;
          res.label = '';
          res.zIndex = 0;
        } else {
          res.zIndex = 1;
          if (f?.has(id) || (q && matches(id))) res.forceLabel = true;
        }
        if (editing && layout.pinned[id]) { res.label = `📌 ${a.label}`; res.forceLabel = true; }
        if (id === selected) { res.highlighted = true; res.zIndex = 2; }
        return res;
      },
      edgeReducer: (id, a) => {
        const res = { ...a };
        const [s, t] = graph.extremities(id);
        if (hidden[graph.getNodeAttribute(s, 'kind')] || hidden[graph.getNodeAttribute(t, 'kind')]) { res.hidden = true; return res; }
        const f = focusSet();
        if (f && !(f.has(s) && f.has(t) && (s === (hovered ?? selected) || t === (hovered ?? selected)))) res.color = '#111823';
        else if (f) res.color = a.kind === 'backed_up_to' ? '#2dd4bf' : '#5b6b84';
        return res;
      },
    });

    r.on('beforeRender', () => (occupied.length = 0));
    let drag = null;
    let moved = false;
    r.on('enterNode', ({ node }) => { if (!node.startsWith('pt:')) hovered = node; });
    r.on('leaveNode', () => (hovered = null));
    r.on('downNode', ({ node }) => {
      if (node.startsWith('pt:') && !editing) return;
      drag = node;
      moved = false;
    });
    r.getMouseCaptor().on('mousemovebody', (e) => {
      if (!drag) return;
      const pos = r.viewportToGraph(e);
      moved = true;
      if (drag.startsWith('pt:')) {
        const p = pointAtPin(drag);
        if (p) { p.x = pos.x; p.y = pos.y; graph.mergeNodeAttributes(drag, { x: pos.x, y: pos.y }); engine.update(0.3); }
      } else {
        engine.pin(drag, pos.x, pos.y);
        engine.sim.alphaTarget(0.3).restart();
      }
      e.preventSigmaDefault();
      e.original.preventDefault();
      e.original.stopPropagation();
    });
    r.getMouseCaptor().on('mouseup', () => {
      if (!drag) return;
      if (moved) {
        engine.sim.alphaTarget(0);
        pinnedCount = Object.keys(layout.pinned).length;
        markDirty();
      }
      drag = null;
    });
    r.on('clickNode', ({ node }) => {
      if (moved) { moved = false; return; }
      if (node.startsWith('pt:')) { selectedPoint = node.slice(3); return; }
      selected = node;
    });
    r.on('clickStage', ({ event }) => {
      if (placing) {
        const pos = r.viewportToGraph({ x: event.x, y: event.y });
        addPoint(placing, pos.x, pos.y);
        placing = null;
        return;
      }
      selected = null;
      selectedPoint = null;
    });
    r.on('doubleClickNode', ({ node, event }) => { event.preventSigmaDefault(); flyTo(node); });
    return r;
  }

  onMount(() => {
    let alive = true;
    invoke('load_layout').then((saved) => { if (alive) start(saved); });
    return () => { alive = false; stopAll(); };
  });

  const kindName = (k) => KINDS[k]?.label?.replace(/s$/, '') ?? k;
  const fmtNum = (v) => (Math.abs(v) < 1 ? v.toFixed(2) : Math.round(v));
</script>

<div class="wrap" class:placing>
  <div class="canvas" bind:this={container}></div>

  <div class="tools">
    <input placeholder="Search the map…  (Enter to jump)" bind:value={query} onkeydown={onSearchKey} />
    <button title="Fit the whole map" onclick={resetView}>⤢</button>
    <button class="edit" class:on={editing} onclick={() => { editing = !editing; placing = null; }}>
      {editing ? 'Done editing' : '✎ Edit layout'}{dirty ? ' •' : ''}
    </button>
  </div>

  {#if placing}
    <div class="hint">Click anywhere on the map to place a <b>{POINT_TYPES[placing].label}</b> point · <button onclick={() => (placing = null)}>Cancel</button></div>
  {/if}

  <div class="legend">
    {#each Object.entries(KINDS) as [k, v] (k)}
      {#if counts[k]}
        <button class:off={hidden[k]} onclick={() => (hidden = { ...hidden, [k]: !hidden[k] })}>
          <span class="dot" style="background:{v.color}"></span>{v.label}<em>{counts[k]}</em>
        </button>
      {/if}
    {/each}
    <span class="warnkey"><span class="dot" style="background:{WARN}"></span>Not backed up</span>
  </div>
</div>

<section class="panel">
  {#if editing}
    <div class="kind" style="color:#fbbf24">Layout editor</div>
    <p class="muted small">Drag any dot to pin it. Drag the colored points to move them. Changes apply live; <b>Save</b> keeps them for good.</p>
    <div class="row">
      <button class="primary" disabled={!dirty || saving} onclick={save}>{saving ? 'Saving…' : 'Save layout'}</button>
      <button disabled={saving} onclick={revert}>Revert</button>
      <button disabled={saving} onclick={resetDefault}>Default</button>
    </div>
    {#if savedNote}<div class="note">{savedNote}</div>{/if}

    <h3>Points</h3>
    {#each layout.points as p (p.id)}
      <div class="pt" class:sel={selectedPoint === p.id}>
        <div class="pthead">
          <span class="dot" style="background:{POINT_TYPES[p.type].color}"></span>
          <select value={p.type} onchange={(e) => setPointType(p, e.currentTarget.value)}>
            {#each Object.entries(POINT_TYPES) as [k, v] (k)}<option value={k}>{v.label}</option>{/each}
          </select>
          <button class="x" title="Remove point" onclick={() => removePoint(p.id)}>✕</button>
        </div>
        <label>Strength <span>{fmtNum(p.strength)}</span>
          <input type="range" min="0" max="3" step="0.05" bind:value={p.strength} oninput={() => changed(0.4)} />
        </label>
        {#if p.type !== 'radiate'}
          <label>Reach <span>{p.reach ? Math.round(p.reach) : 'everywhere'}</span>
            <input type="range" min="0" max="1500" step="10" bind:value={p.reach} oninput={() => changed(0.4)} />
          </label>
        {/if}
      </div>
    {/each}
    <div class="row">
      {#each Object.entries(POINT_TYPES) as [k, v] (k)}
        <button class:armed={placing === k} onclick={() => (placing = placing === k ? null : k)}>
          <span class="dot" style="background:{v.color}"></span>+ {v.label}
        </button>
      {/each}
    </div>

    <h3>Forces</h3>
    {#each SLIDERS as s (s.key)}
      <label class="slider">{s.label} <span>{fmtNum(layout.settings[s.key])}</span>
        <input type="range" min={s.min} max={s.max} step={s.step} bind:value={layout.settings[s.key]} oninput={() => changed(0.5)} />
      </label>
    {/each}

    <h3>Pinned dots</h3>
    <p class="muted small">{pinnedCount} pinned{pinnedCount ? '' : ' · drag any dot to pin it in place'}</p>
    {#if pinnedCount}<div class="row"><button onclick={unpinAll}>Unpin all</button></div>{/if}
  {:else if sel}
    <div class="kind" style="color:{KINDS[sel.kind]?.color}">{kindName(sel.kind)}</div>
    <h2>{sel.label}</h2>
    {#if sel.detail}<div class="sub">{sel.detail}</div>{/if}
    <dl>
      <dt>Size</dt><dd>{fmtBytes(sel.size)}</dd>
      {#if sel.newest}<dt>Last changed</dt><dd>{fmtDate(sel.newest)} <span class="muted">({fmtAgo(sel.newest)})</span></dd>{/if}
      {#if sel.backed_up != null}
        <dt>Backed up</dt>
        <dd>
          {#if sel.backed_up}<span class="ok">✓ {sel.backups.join(' · ')}</span>{:else}<span class="bad">Not in any backup</span>{/if}
        </dd>
      {/if}
      {#if sel.path}<dt>Path</dt><dd class="path">{sel.path}</dd>{/if}
    </dl>
    <div class="actions">
      {#if sel.scan_id && sel.dir_id}<button onclick={() => onspace?.(sel.scan_id, sel.dir_id)}>Open in Space view</button>{/if}
      {#if sel.path && sel.kind !== 'drive'}<button onclick={() => onreveal?.(sel.path)}>Show in Explorer</button>{/if}
      {#if pinnedCount >= 0 && layout.pinned[sel.id]}<button onclick={unpinSelected}>📌 Unpin</button>{/if}
    </div>
    {#if neighbors.up.length}
      <h3>Lives in</h3>
      {#each neighbors.up as n (n.id)}
        <button class="nb" onclick={() => { selected = n.id; flyTo(n.id); }}><span class="dot" style="background:{colorOf(n)}"></span>{n.label}</button>
      {/each}
    {/if}
    {#if neighbors.backups.length}
      <h3>Backed up to</h3>
      {#each neighbors.backups as n (n.id)}
        <button class="nb" onclick={() => { selected = n.id; flyTo(n.id); }}><span class="dot" style="background:{colorOf(n)}"></span>{n.label}</button>
      {/each}
    {/if}
    {#if neighbors.down.length}
      <h3>{sel.kind === 'backup' ? 'Backs up' : 'Contains'}</h3>
      {#each neighbors.down as n (n.id)}
        <button class="nb" onclick={() => { selected = n.id; flyTo(n.id); }}>
          <span class="dot" style="background:{colorOf(n)}"></span><span class="nm">{n.label}</span><span class="sz">{fmtBytes(n.size)}</span>
        </button>
      {/each}
      {#if neighbors.more}<div class="muted small">+ {neighbors.more} more</div>{/if}
    {/if}
  {:else}
    <h3 class="first">Map</h3>
    <p class="muted">Every drive, project, game and app on your PC, and where they're backed up.</p>
    <ul class="tips">
      <li><b>Click</b> a dot for details, <b>double-click</b> to zoom in.</li>
      <li><b>Drag</b> a dot to pin it; drag empty space to move around.</li>
      <li><b>Search</b> to find anything; Enter jumps to it.</li>
      <li><b>✎ Edit layout</b> to change the forces and where things radiate from.</li>
      <li><span class="bad">Red</span> projects aren't in any backup.</li>
    </ul>
    <p class="muted small">{data.nodes.length} nodes · built in {(data.built_ms / 1000).toFixed(1)} s<br />Backups are set in {data.config_path}</p>
  {/if}
</section>

<style>
  .wrap { position: relative; min-height: 0; overflow: hidden; background: radial-gradient(ellipse at 50% 45%, #101a2a 0%, #0b0f17 70%); }
  .wrap.placing { cursor: crosshair; }
  .canvas { position: absolute; inset: 0; }
  .tools { position: absolute; top: 12px; left: 12px; display: flex; gap: 6px; }
  .tools input { width: 280px; padding: 8px 11px; border-radius: 9px; border: 1px solid #22304a; background: rgba(13,19,30,0.9); color: var(--text); font: inherit; font-size: 13px; outline: none; }
  .tools input:focus { border-color: var(--accent); }
  .tools button { min-width: 34px; padding: 0 10px; border-radius: 9px; border: 1px solid #22304a; background: rgba(13,19,30,0.9); text-align: center; color: var(--muted); font-size: 12.5px; }
  .tools button:hover { color: var(--accent); border-color: var(--accent); }
  .tools button.edit.on { color: #fbbf24; border-color: #fbbf24; }
  .hint { position: absolute; top: 58px; left: 12px; padding: 7px 11px; border-radius: 8px; background: rgba(40,30,8,0.92); border: 1px solid #a16207; font-size: 12.5px; color: #fde68a; }
  .hint button { color: #fbbf24; text-decoration: underline; }
  .legend { position: absolute; left: 12px; bottom: 12px; display: flex; flex-wrap: wrap; gap: 6px; max-width: calc(100% - 24px); }
  .legend button, .warnkey { display: flex; align-items: center; gap: 6px; padding: 5px 9px; border-radius: 8px; background: rgba(13,19,30,0.88); border: 1px solid #1f2a3a; font-size: 12px; }
  .legend button:hover { border-color: #33445f; }
  .legend button.off { opacity: 0.4; }
  .legend em { font-style: normal; color: var(--muted); font-size: 11px; }
  .dot { width: 9px; height: 9px; border-radius: 50%; flex: none; display: inline-block; }

  .panel { border-left: 1px solid var(--line); padding: 16px 16px 20px; overflow-y: auto; overflow-x: hidden; min-height: 0; font-size: 13px; }
  .panel .small { overflow-wrap: anywhere; }
  .kind { font-size: 11px; text-transform: uppercase; letter-spacing: 1.2px; font-weight: 600; }
  h2 { margin: 4px 0 2px; font-size: 18px; word-break: break-word; }
  .sub { color: var(--muted); font-size: 12.5px; }
  dl { display: grid; grid-template-columns: auto 1fr; gap: 6px 12px; margin: 14px 0; font-size: 12.5px; }
  dt { color: var(--muted); }
  dd { margin: 0; word-break: break-word; }
  .path { color: #8aa0bd; font-size: 11.5px; }
  .ok { color: #5eead4; }
  .bad { color: #f87171; }
  .muted { color: var(--muted); }
  .small { font-size: 11.5px; line-height: 1.5; }
  .actions { display: flex; gap: 6px; flex-wrap: wrap; margin-bottom: 6px; }
  .actions button, .row button { padding: 6px 10px; border-radius: 7px; border: 1px solid var(--line); font-size: 12px; color: var(--accent); display: inline-flex; align-items: center; gap: 6px; }
  .actions button:hover, .row button:not(:disabled):hover { border-color: var(--accent); background: #12302f; }
  .row { display: flex; gap: 6px; flex-wrap: wrap; margin: 8px 0; }
  .row button:disabled { opacity: 0.45; }
  .row button.primary { background: #134e4a; border-color: #2dd4bf; color: #ccfbf1; }
  .row button.armed { border-color: #fbbf24; color: #fbbf24; background: #2a2108; }
  .note { font-size: 12px; color: #5eead4; margin: 2px 0 6px; }
  h3 { font-size: 11px; text-transform: uppercase; letter-spacing: 1.2px; color: var(--muted); margin: 16px 0 6px; }
  h3.first { margin-top: 2px; }
  .nb { display: flex; align-items: center; gap: 8px; width: 100%; padding: 5px 6px; border-radius: 6px; font-size: 12.5px; }
  .nb:hover { background: var(--panel); }
  .nb .nm { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .nb .sz { color: var(--muted); font-size: 11.5px; }
  .tips { padding-left: 16px; font-size: 12.5px; line-height: 1.7; color: #b8c4d6; }
  .pt { padding: 8px 9px; border-radius: 8px; border: 1px solid var(--line); margin-bottom: 6px; }
  .pt.sel { border-color: #fbbf24; }
  .pthead { display: flex; align-items: center; gap: 8px; margin-bottom: 4px; }
  .pthead select { flex: 1; background: var(--panel); color: var(--text); border: 1px solid var(--line); border-radius: 6px; padding: 4px 6px; font: inherit; font-size: 12.5px; }
  .x { color: var(--muted); width: 24px; height: 24px; border-radius: 6px; text-align: center; }
  .x:hover { color: #f87171; background: #2a1215; }
  label { display: block; font-size: 12px; color: #b8c4d6; margin: 6px 0 2px; }
  label span { float: right; color: var(--muted); }
  label input[type='range'] { width: 100%; accent-color: #2dd4bf; margin: 3px 0 0; }
</style>
