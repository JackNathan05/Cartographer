<script>
  import Graph from 'graphology';
  import Sigma from 'sigma';
  import { onMount } from 'svelte';
  import { fmtBytes, fmtAgo, fmtDate } from './format.js';

  let { data, onspace, onreveal } = $props();

  const KINDS = {
    drive: { label: 'Drives', color: '#e2e8f0' },
    folder: { label: 'Folders', color: '#3b4a61' },
    project: { label: 'Projects', color: '#38bdf8' },
    game: { label: 'Games', color: '#4ade80' },
    app: { label: 'Apps', color: '#a78bfa' },
    service: { label: 'Services', color: '#fb923c' },
    backup: { label: 'Backups', color: '#2dd4bf' },
  };
  const WARN = '#f87171';
  const DIM = '#1a2333';

  let container;
  let renderer = null;
  let graph = null;
  let hovered = $state(null);
  let selected = $state(null);
  let query = $state('');
  let hidden = $state({}); // kind -> true when hidden
  let counts = $state({});

  const byId = $derived(new Map(data.nodes.map((n) => [n.id, n])));
  const sel = $derived(selected ? byId.get(selected) : null);

  function hash(s) {
    let h = 2166136261;
    for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619);
    return ((h >>> 0) % 100000) / 100000;
  }

  function nodeSize(n) {
    if (n.kind === 'drive') return 18;
    if (n.kind === 'backup') return 12;
    const gb = Math.max(n.size, 0) / 1073741824;
    const s = Math.min(22, 3 + Math.sqrt(gb) * 2.2);
    return n.kind === 'folder' ? Math.max(3, s * 0.6) : s; // folders are structure, keep them quiet
  }

  function colorOf(n) {
    if ((n.kind === 'project' || n.kind === 'folder') && n.backed_up === false && n.kind === 'project') return WARN;
    return KINDS[n.kind]?.color ?? '#94a3b8';
  }

  function build() {
    const g = new Graph({ multi: false, type: 'directed' });
    const c = {};
    for (const n of data.nodes) {
      c[n.kind] = (c[n.kind] ?? 0) + 1;
      g.addNode(n.id, {
        label: n.label,
        size: nodeSize(n),
        color: colorOf(n),
        kind: n.kind,
        forceLabel: n.kind === 'drive' || n.kind === 'backup',
        x: 0,
        y: 0,
      });
    }
    counts = c;
    for (const l of data.links) {
      if (!g.hasNode(l.source) || !g.hasNode(l.target) || g.hasEdge(l.source, l.target)) continue;
      const backup = l.kind === 'backed_up_to';
      g.addEdge(l.source, l.target, {
        kind: l.kind,
        color: backup ? 'rgba(45,212,191,0.55)' : '#243045',
        size: backup ? 1.4 : 1,
        weight: backup ? 0.15 : 1,
      });
    }

    // Radial layout: each drive is the center of its own circle, and its folders,
    // projects, apps and games fan outward in rings (one ring per folder level).
    // Each branch gets a slice of the circle sized by how many things are in it.
    // Drives sit in a compact grid in the middle. Deterministic: same map every time.
    const kidsOf = new Map();
    g.forEachEdge((e, a, s, t) => {
      if (a.kind !== 'lives_on') return;
      if (!kidsOf.has(t)) kidsOf.set(t, []);
      kidsOf.get(t).push(s);
    });
    for (const list of kidsOf.values()) list.sort((x, y) => (byId.get(y)?.size ?? 0) - (byId.get(x)?.size ?? 0) || x.localeCompare(y));
    const weight = new Map();
    const depthOf = new Map();
    const measure = (id, d) => {
      depthOf.set(id, d);
      const kids = kidsOf.get(id) ?? [];
      let leaves = kids.length ? 0 : 1;
      for (const k of kids) leaves += measure(k, d + 1);
      // slices grow with the square root of what's inside, so one huge branch (Users)
      // can't take half the circle and small branches still get room to fan out
      weight.set(id, Math.sqrt(leaves) + 0.5);
      return leaves;
    };
    const RING = 52;
    const ringR = (d) => RING * Math.pow(d, 0.9) + (d > 0 ? 18 : 0);
    const local = new Map(); // id -> {x, y} relative to its drive
    const place = (id, a0, a1, d, nth = 0) => {
      const mid = (a0 + a1) / 2;
      // end-of-branch dots alternate between two radii so a crowded arc becomes a zig-zag
      const leaf = !(kidsOf.get(id) ?? []).length;
      const r = ringR(d) + (leaf && d > 1 ? (nth % 2) * 16 : 0);
      local.set(id, { x: Math.cos(mid) * r, y: Math.sin(mid) * r });
      const kids = kidsOf.get(id) ?? [];
      const total = kids.reduce((s2, k) => s2 + weight.get(k), 0) || 1;
      const span = a1 - a0;
      let cur = a0;
      for (const [i, k] of kids.entries()) {
        const w = (weight.get(k) / total) * span;
        place(k, cur, cur + w, d + 1, i);
        cur += w;
      }
    };
    const maxDepth = (id) => 1 + Math.max(0, ...(kidsOf.get(id) ?? []).map(maxDepth));
    const allDrives = data.nodes.filter((n) => n.kind === 'drive');
    for (const dv of allDrives) measure(dv.id, 0);
    const drives = allDrives.sort((x, y) => maxDepth(y.id) * 1000 + weight.get(y.id) - (maxDepth(x.id) * 1000 + weight.get(x.id)));
    const radius = new Map();
    for (const dv of drives) {
      place(dv.id, 0, Math.PI * 2, 0);
      radius.set(dv.id, ringR(Math.max(1, maxDepth(dv.id) - 1)));
    }
    // drives packed around the middle (2x2 for four drives), biggest first; each sits just
    // far enough out that its circle clears its neighbours'
    const GAP = 50;
    const n = drives.length;
    const driveOf = new Map();
    const tag = (id, dv) => { driveOf.set(id, dv); for (const k of kidsOf.get(id) ?? []) tag(k, dv); };
    g.__center = {};
    drives.forEach((dv, i) => {
      const ang = -Math.PI * 3 / 4 + (i * 2 * Math.PI) / n;
      const dist = n === 1 ? 0 : (radius.get(dv.id) + GAP) / Math.sin(Math.PI / n);
      const cx = Math.cos(ang) * dist;
      const cy = Math.sin(ang) * dist;
      tag(dv.id, dv.id);
      g.__center[dv.id] = [cx, cy];
    });
    g.forEachNode((id) => {
      const dv = driveOf.get(id);
      const p = local.get(id);
      if (dv && p && id !== dv) {
        const [cx, cy] = g.__center[dv];
        g.mergeNodeAttributes(id, { x: cx + p.x, y: -(cy + p.y) });
      } else if (!dv) {
        g.mergeNodeAttributes(id, { x: (hash(id) - 0.5) * 50, y: (hash(id + '!') - 0.5) * 50 });
      }
    });
    for (const dv of drives) {
      const [cx, cy] = g.__center[dv.id];
      g.mergeNodeAttributes(dv.id, { x: cx, y: -cy });
    }
    return g;
  }

  // dark hover card instead of sigma's default white box
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
    if (!f || !graph?.hasNode(f)) return null;
    const s = new Set([f]);
    graph.forEachNeighbor(f, (n) => s.add(n));
    return s;
  }

  function matches(id) {
    const q = query.trim().toLowerCase();
    if (!q) return true;
    const n = byId.get(id);
    return n.label.toLowerCase().includes(q) || (n.path ?? '').toLowerCase().includes(q);
  }

  function refresh() {
    renderer?.refresh({ skipIndexation: true });
  }

  $effect(() => {
    // re-render when interaction state changes
    hovered; selected; query; hidden;
    refresh();
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
    renderer?.getCamera().animate({ x: 0.5, y: 0.5, ratio: 1, angle: 0 }, { duration: 500 });
  }

  const neighbors = $derived.by(() => {
    if (!sel || !graph?.hasNode(sel.id)) return { up: [], down: [], backups: [] };
    const up = [], down = [], backups = [];
    graph.forEachOutEdge(sel.id, (e, a, s, t) => (a.kind === 'backed_up_to' ? backups : up).push(byId.get(t)));
    graph.forEachInEdge(sel.id, (e, a, s) => { if (a.kind === 'lives_on') down.push(byId.get(s)); else down.push(byId.get(s)); });
    down.sort((a, b) => b.size - a.size);
    return { up, down: down.slice(0, 12), more: Math.max(0, down.length - 12), backups };
  });

  onMount(() => {
    graph = build();
    renderer = new Sigma(graph, container, {
      labelColor: { color: '#cbd5e1' },
      labelFont: "'Segoe UI Variable Text', 'Segoe UI', sans-serif",
      labelSize: 12,
      labelWeight: '500',
      labelDensity: 0.45,
      labelGridCellSize: 120,
      labelRenderedSizeThreshold: 7,
      defaultDrawNodeHover: drawHover,
      zIndex: true,
      stagePadding: 70,
      minCameraRatio: 0.05,
      maxCameraRatio: 4,
      nodeReducer: (id, a) => {
        const r = { ...a };
        if (hidden[a.kind]) { r.hidden = true; return r; }
        const f = focusSet();
        const q = query.trim();
        if ((f && !f.has(id)) || (q && !matches(id))) {
          r.color = DIM;
          r.label = '';
          r.zIndex = 0;
        } else {
          r.zIndex = 1;
          if (f?.has(id) || (q && matches(id))) r.forceLabel = true;
        }
        if (id === selected) { r.highlighted = true; r.zIndex = 2; }
        return r;
      },
      edgeReducer: (id, a) => {
        const r = { ...a };
        const [s, t] = graph.extremities(id);
        if (hidden[graph.getNodeAttribute(s, 'kind')] || hidden[graph.getNodeAttribute(t, 'kind')]) { r.hidden = true; return r; }
        const f = focusSet();
        if (f && !(f.has(s) && f.has(t) && (s === (hovered ?? selected) || t === (hovered ?? selected)))) r.color = '#111823';
        else if (f) r.color = a.kind === 'backed_up_to' ? '#2dd4bf' : '#5b6b84';
        return r;
      },
    });
    renderer.on('enterNode', ({ node }) => (hovered = node));
    renderer.on('leaveNode', () => (hovered = null));
    renderer.on('clickNode', ({ node }) => (selected = node));
    renderer.on('clickStage', () => (selected = null));
    renderer.on('doubleClickNode', ({ node, event }) => { event.preventSigmaDefault(); flyTo(node); });
    return () => renderer?.kill();
  });

  const kindName = (k) => KINDS[k]?.label?.replace(/s$/, '') ?? k;
</script>

<div class="wrap">
  <div class="canvas" bind:this={container}></div>

  <div class="tools">
    <input placeholder="Search the map…  (Enter to jump)" bind:value={query} onkeydown={onSearchKey} />
    <button title="Reset view" onclick={resetView}>⤢</button>
  </div>

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
  {#if sel}
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
      <li><b>Scroll</b> to zoom, <b>drag</b> to move around.</li>
      <li><b>Search</b> to find anything; Enter jumps to it.</li>
      <li>Click the legend to show or hide a kind.</li>
      <li><span class="bad">Red</span> projects aren't in any backup.</li>
    </ul>
    <p class="muted small">{data.nodes.length} nodes · built in {(data.built_ms / 1000).toFixed(1)} s<br />Backups are set in {data.config_path}</p>
  {/if}
</section>

<style>
  .wrap { position: relative; min-height: 0; overflow: hidden; background: radial-gradient(ellipse at 50% 40%, #101a2a 0%, #0b0f17 70%); }
  .canvas { position: absolute; inset: 0; }
  .tools { position: absolute; top: 12px; left: 12px; display: flex; gap: 6px; }
  .tools input { width: 300px; padding: 8px 11px; border-radius: 9px; border: 1px solid #22304a; background: rgba(13,19,30,0.9); color: var(--text); font: inherit; font-size: 13px; outline: none; }
  .tools input:focus { border-color: var(--accent); }
  .tools button { width: 34px; border-radius: 9px; border: 1px solid #22304a; background: rgba(13,19,30,0.9); text-align: center; color: var(--muted); }
  .tools button:hover { color: var(--accent); border-color: var(--accent); }
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
  .actions button { padding: 6px 10px; border-radius: 7px; border: 1px solid var(--line); font-size: 12px; color: var(--accent); }
  .actions button:hover { border-color: var(--accent); background: #12302f; }
  h3 { font-size: 11px; text-transform: uppercase; letter-spacing: 1.2px; color: var(--muted); margin: 16px 0 6px; }
  h3.first { margin-top: 2px; }
  .nb { display: flex; align-items: center; gap: 8px; width: 100%; padding: 5px 6px; border-radius: 6px; font-size: 12.5px; }
  .nb:hover { background: var(--panel); }
  .nb .nm { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .nb .sz { color: var(--muted); font-size: 11.5px; }
  .tips { padding-left: 16px; font-size: 12.5px; line-height: 1.7; color: #b8c4d6; }
</style>
