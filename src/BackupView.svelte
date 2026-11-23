<script>
  import { fmtBytes, fmtAgo, fmtDate } from './format.js';
  import { privacy, pv } from './privacy.svelte.js';

  let { report, onspace, onreveal } = $props();

  const DEST_COLORS = ['#2dd4bf', '#fbbf24', '#a78bfa', '#38bdf8', '#f472b6'];
  const destColor = (i) => DEST_COLORS[i % DEST_COLORS.length];
  const DAY = 86400;

  function freshness(ts) {
    if (!ts) return { label: 'Unknown', cls: 'unk' };
    const age = Date.now() / 1000 - ts;
    if (age < 8 * DAY) return { label: 'Up to date', cls: 'ok' };
    if (age < 31 * DAY) return { label: 'Getting old', cls: 'warn' };
    return { label: 'Out of date', cls: 'bad' };
  }

  const isReplaceable = (f) => /Games|re-download|reinstallable/.test(f.hint ?? '');
  // your at-risk folders first, then the re-downloadable / reinstallable ones
  const unprotected = $derived(
    report.folders.filter((f) => !f.backups.length).sort((a, b) => isReplaceable(a) - isReplaceable(b) || b.size - a.size),
  );
  const protectedList = $derived(report.folders.filter((f) => f.backups.length));
  const risky = $derived(unprotected.filter((f) => !isReplaceable(f) && !(f.hint ?? '').includes('partly')));
  const riskyBytes = $derived(risky.reduce((s, f) => s + f.size, 0));
  const nameIndex = $derived(new Map(report.destinations.map((d, i) => [d.name, i])));

  // drive bars: split "not backed up" into your files vs Windows / apps / games
  const bars = $derived(
    report.coverage.map((c) => {
      const mine = risky.filter((f) => f.path.toUpperCase().startsWith(c.letter + ':')).reduce((s, f) => s + f.size, 0);
      const yours = Math.min(mine, c.none);
      return { ...c, yours, rest: Math.max(0, c.none - yours) };
    }),
  );
  const twiceTotal = $derived(report.coverage.reduce((s, c) => s + c.twice, 0));
  const onceTotal = $derived(report.coverage.reduce((s, c) => s + c.once, 0));

  // ---- flow diagram (sources -> destinations)
  const W = 900;
  const flow = $derived.by(() => {
    const srcMap = new Map();
    report.destinations.forEach((d, di) => {
      for (const s of d.sources) {
        const k = s.path.toLowerCase();
        if (!srcMap.has(k)) srcMap.set(k, { ...s, dests: [] });
        srcMap.get(k).dests.push(di);
      }
    });
    const sources = [...srcMap.values()].sort((a, b) => b.size - a.size);
    const total = sources.reduce((s, x) => s + x.size * x.dests.length, 0) || 1;
    const avail = Math.max(240, sources.length * 34);
    const gap = 10;
    const scale = (avail - gap * sources.length) / total;
    const th = (bytes) => Math.max(4, bytes * scale);

    let y = 10;
    const left = sources.map((s) => {
      const parts = s.dests.map((di) => ({ di, h: th(s.size) }));
      const h = parts.reduce((a, p) => a + p.h, 0);
      const node = { ...s, y, h, parts };
      y += Math.max(h, 26) + gap;
      return node;
    });
    const height = Math.max(y, 200) + 10;

    // destination stacks, centered vertically
    const destH = report.destinations.map((_, di) => left.reduce((a, s) => a + s.parts.filter((p) => p.di === di).reduce((b, p) => b + p.h, 0), 0));
    const dgap = 40;
    const totalD = destH.reduce((a, b) => a + b, 0) + dgap * (destH.length - 1);
    let dy = Math.max(10, (height - totalD) / 2);
    const right = report.destinations.map((d, di) => {
      const node = { ...d, di, y: dy, h: destH[di] };
      dy += destH[di] + dgap;
      return node;
    });

    // ribbons
    const cursorL = new Map(left.map((s) => [s.path, s.y]));
    const cursorR = right.map((r) => r.y);
    const x0 = 250, x1 = W - 250;
    const ribbons = [];
    for (const s of left) {
      for (const p of s.parts) {
        const ly = cursorL.get(s.path);
        const ry = cursorR[p.di];
        cursorL.set(s.path, ly + p.h);
        cursorR[p.di] = ry + p.h;
        const mx = (x0 + x1) / 2;
        const d = `M${x0},${ly} C${mx},${ly} ${mx},${ry} ${x1},${ry} L${x1},${ry + p.h} C${mx},${ry + p.h} ${mx},${ly + p.h} ${x0},${ly + p.h} Z`;
        ribbons.push({ d, di: p.di, src: s.path, size: s.size });
      }
    }
    return { left, right, ribbons, height, x0, x1 };
  });

  let hoverSrc = $state(null);
  let hoverDest = $state(null);
  const ribbonOn = (r) => (hoverSrc ? r.src === hoverSrc : hoverDest != null ? r.di === hoverDest : true);
</script>

<div class="bv">
  <div class="intro">
    <h1>Backups</h1>
    <p>
      {#if report.destinations.length === 0}
        No backups are set up in Cartographer yet. Add them to <code>{report.config_path}</code>.
      {:else}
        <b>{fmtBytes(twiceTotal)}</b> of your files is in two backups and <b>{fmtBytes(onceTotal)}</b> is in one.
        {#if riskyBytes > 0}
          <span class="bad"><b>{fmtBytes(riskyBytes)}</b> in {risky.length} of your folders isn't backed up anywhere.</span>
        {:else}
          <span class="okc">All of your own folders are backed up.</span>
        {/if}
      {/if}
    </p>
  </div>

  <!-- 1. where backups go -->
  <section>
    <h2>Where your backups go</h2>
    <div class="cards">
      {#each report.destinations as d, i (d.name)}
        {@const f = freshness(d.last_activity)}
        {@const srcTotal = d.sources.reduce((s, x) => s + x.size, 0)}
        <div class="card" style="--c:{destColor(i)}">
          <div class="top">
            <span class="swatch"></span>
            <h3>{d.name}</h3>
            <span class="pill {d.connected && d.target_exists ? f.cls : 'bad'}">
              {d.connected ? (d.target_exists ? f.label : 'Folder missing') : 'Not connected'}
            </span>
          </div>
          <div class="where">{d.drive}: {d.drive_label} · <span class="path" class:pvt={privacy.on}>{pv(d.target)}</span></div>
          <div class="stats">
            <div><span>Last activity</span><b>{d.last_activity ? fmtAgo(d.last_activity) : '—'}</b><em>{d.last_activity ? fmtDate(d.last_activity) : ''}</em></div>
            <div><span>Backs up</span><b>{d.sources.length} folder{d.sources.length === 1 ? '' : 's'}</b><em>{fmtBytes(srcTotal)} on your PC</em></div>
            <div><span>Stored there</span><b>{fmtBytes(d.stored)}</b><em>as of the last scan</em></div>
          </div>
          {#if d.drive_total}
            <div class="space">
              <div class="bar"><div style="width:{(1 - d.drive_free / d.drive_total) * 100}%"></div></div>
              <span>{fmtBytes(d.drive_free)} free of {fmtBytes(d.drive_total)} on {d.drive}:</span>
            </div>
          {/if}
        </div>
      {/each}
    </div>
  </section>

  <!-- 2. what goes where -->
  {#if report.destinations.length}
    <section>
      <h2>What goes where</h2>
      <svg class="flow" viewBox="0 0 {W} {flow.height}" style="height:{flow.height}px">
        {#each flow.ribbons as r, i (i)}
          <path d={r.d} fill={destColor(r.di)} opacity={ribbonOn(r) ? 0.38 : 0.06} />
        {/each}
        {#each flow.left as s (s.path)}
          <g class="src" role="presentation" onmouseenter={() => (hoverSrc = s.path)} onmouseleave={() => (hoverSrc = null)}
             onclick={() => s.scan_id && onspace?.(s.scan_id, s.dir_id)}>
            <rect x={flow.x0 - 8} y={s.y} width="8" height={s.h} rx="2" fill="#cbd5e1" />
            <text x={flow.x0 - 16} y={s.y + Math.max(s.h, 14) / 2 - 2} text-anchor="end" class="lbl" class:pvt={privacy.on}>{pv(s.label)}<tspan class="sz" dx="6">{fmtBytes(s.size)}</tspan></text>
            <text x={flow.x0 - 16} y={s.y + Math.max(s.h, 14) / 2 + 12} text-anchor="end" class="sub" class:pvt={privacy.on}>{pv(s.path)}</text>
          </g>
        {/each}
        {#each flow.right as d (d.name)}
          <g role="presentation" onmouseenter={() => (hoverDest = d.di)} onmouseleave={() => (hoverDest = null)}>
            <rect x={flow.x1} y={d.y} width="10" height={Math.max(d.h, 4)} rx="2" fill={destColor(d.di)} />
            <text x={flow.x1 + 18} y={d.y + d.h / 2 - 2} class="lbl">{d.name}</text>
            <text x={flow.x1 + 18} y={d.y + d.h / 2 + 13} class="sub">{d.drive}: {d.drive_label}</text>
          </g>
        {/each}
      </svg>
      <p class="note">Each band is a folder going into a backup; its thickness is the folder's size. Click a folder to see it in Space view.</p>
    </section>
  {/if}

  <!-- 3. coverage by drive -->
  <section>
    <h2>How much of each drive is protected</h2>
    <div class="legend">
      <span><i style="background:#22c55e"></i>In 2+ backups</span>
      <span><i style="background:#2dd4bf"></i>In 1 backup</span>
      <span><i style="background:#f87171"></i>Your files, not backed up</span>
      <span><i class="hatch"></i>Windows, apps &amp; games (reinstallable)</span>
    </div>
    {#each bars as c (c.letter)}
      <div class="drow">
        <div class="dname"><span><b>{c.letter}:</b> {c.label || 'Local disk'}</span><em>{fmtBytes(c.used)} used</em></div>
        <div class="stack">
          {#each [['twice', '#22c55e'], ['once', '#2dd4bf'], ['yours', '#f87171'], ['rest', null]] as [k, col] (k)}
            {#if c[k] > 0}
              <div class="seg" class:hatch={!col} style="width:{(c[k] / c.used) * 100}%; {col ? `background:${col}` : ''}" title="{fmtBytes(c[k])}"></div>
            {/if}
          {/each}
        </div>
        <div class="nums">
          {#if c.twice}<span style="color:#4ade80">{fmtBytes(c.twice)}</span>{/if}
          {#if c.once}<span style="color:#5eead4">{fmtBytes(c.once)}</span>{/if}
          {#if c.yours}<span style="color:#f87171">{fmtBytes(c.yours)}</span>{/if}
          <span class="muted">{fmtBytes(c.rest)}</span>
        </div>
      </div>
    {/each}
  </section>

  <!-- 4. folder lists -->
  <section class="lists">
    <div>
      <h2>Not backed up</h2>
      <ul>
        {#each unprotected as f (f.path)}
          {@const replaceable = isReplaceable(f)}
          <li>
            <button onclick={() => f.scan_id && onspace?.(f.scan_id, f.dir_id)}>
              <span class="dot" style="background:{replaceable ? '#475569' : '#f87171'}"></span>
              <span class="nm" class:pvt={privacy.on}>{pv(f.label)}<small>{pv(f.path)}</small></span>
              {#if f.hint}<span class="chip" class:soft={replaceable}>{f.hint}</span>{/if}
              <span class="sz">{fmtBytes(f.size)}</span>
            </button>
            <button class="rv" title="Show in Explorer" onclick={() => onreveal?.(f.path)}>↗</button>
          </li>
        {:else}
          <li class="empty">Nothing. Every folder is in at least one backup.</li>
        {/each}
      </ul>
    </div>
    <div>
      <h2>Backed up</h2>
      <ul>
        {#each protectedList as f (f.path)}
          <li>
            <button onclick={() => f.scan_id && onspace?.(f.scan_id, f.dir_id)}>
              <span class="dot" style="background:#22c55e"></span>
              <span class="nm" class:pvt={privacy.on}>{pv(f.label)}<small>{pv(f.path)}</small></span>
              {#each f.backups as b (b)}<span class="chip tag" style="--c:{destColor(nameIndex.get(b) ?? 0)}">{b}</span>{/each}
              <span class="sz">{fmtBytes(f.size)}</span>
            </button>
          </li>
        {/each}
      </ul>
    </div>
  </section>

  <p class="foot">Backups are read from <code>{report.config_path}</code>. Folders under 50 MB aren't listed.</p>
</div>

<style>
  .bv { grid-column: 1 / -1; overflow-y: auto; padding: 22px 28px 40px; min-height: 0; }
  .intro h1 { margin: 0 0 6px; font-size: 22px; }
  .intro p { margin: 0; color: #b8c4d6; font-size: 14px; line-height: 1.5; max-width: 900px; }
  .bad { color: #fca5a5; }
  .okc { color: #5eead4; }
  section { margin-top: 28px; }
  h2 { font-size: 11.5px; text-transform: uppercase; letter-spacing: 1.3px; color: var(--muted); margin: 0 0 12px; }
  code { font-size: 12px; color: #8aa0bd; }

  .cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(380px, 1fr)); gap: 14px; }
  .card { border: 1px solid var(--line); border-top: 3px solid var(--c); border-radius: 12px; padding: 14px 16px; background: var(--panel2); }
  .top { display: flex; align-items: center; gap: 9px; }
  .swatch { width: 10px; height: 10px; border-radius: 3px; background: var(--c); }
  .top h3 { margin: 0; font-size: 15.5px; flex: 1; }
  .pill { font-size: 11.5px; padding: 3px 9px; border-radius: 99px; border: 1px solid; }
  .pill.ok { color: #5eead4; border-color: #1d4a47; background: #0f1f26; }
  .pill.warn { color: #fcd34d; border-color: #6b5214; background: #241c08; }
  .pill.bad { color: #fca5a5; border-color: #6b2229; background: #2a1215; }
  .pill.unk { color: var(--muted); border-color: var(--line); }
  .where { margin: 6px 0 12px; font-size: 12.5px; color: #b8c4d6; }
  .where .path { color: #7f93ad; }
  .stats { display: grid; grid-template-columns: repeat(3, 1fr); gap: 10px; }
  .stats div { display: flex; flex-direction: column; }
  .stats span { font-size: 11px; color: var(--muted); }
  .stats b { font-size: 15px; margin: 2px 0 1px; }
  .stats em { font-style: normal; font-size: 11px; color: #6b7c95; }
  .space { margin-top: 12px; display: flex; align-items: center; gap: 10px; font-size: 11.5px; color: var(--muted); }
  .space .bar { flex: 1; height: 6px; border-radius: 3px; background: #1b2535; overflow: hidden; }
  .space .bar div { height: 100%; background: var(--c); opacity: 0.8; }

  .flow { width: 100%; max-width: 1100px; display: block; }
  .flow path { transition: opacity 0.15s; }
  .flow .lbl { fill: #e6edf7; font-size: 13px; font-weight: 600; }
  .flow .sz { fill: #8a9bb3; font-weight: 400; font-size: 12px; }
  .flow .sub { fill: #61738c; font-size: 11px; }
  .flow .src { cursor: pointer; }
  .flow .src:hover .lbl { fill: #5eead4; }
  .note, .foot { font-size: 12px; color: var(--muted); margin: 6px 0 0; }
  .foot { margin-top: 26px; }

  .legend { display: flex; flex-wrap: wrap; gap: 16px; font-size: 12px; color: #b8c4d6; margin-bottom: 12px; }
  .legend span { display: flex; align-items: center; gap: 6px; }
  .legend i { width: 12px; height: 12px; border-radius: 3px; display: inline-block; }
  .hatch { background: repeating-linear-gradient(45deg, #334155 0 3px, #1e293b 3px 6px) !important; }
  .drow { display: grid; grid-template-columns: 210px 1fr 260px; align-items: center; gap: 14px; margin: 10px 0; }
  .dname { font-size: 13px; display: flex; flex-direction: column; }
  .dname em { font-style: normal; font-size: 11.5px; color: var(--muted); }
  .stack { display: flex; height: 22px; border-radius: 6px; overflow: hidden; background: #131b28; }
  .seg { height: 100%; border-right: 1px solid #0b0f17; }
  .nums { display: flex; gap: 10px; font-size: 12px; justify-content: flex-end; }
  .muted { color: var(--muted); }

  .lists { display: grid; grid-template-columns: 1fr 1fr; gap: 26px; }
  ul { list-style: none; margin: 0; padding: 0; }
  li { display: flex; align-items: center; border-radius: 8px; }
  li:hover { background: var(--panel); }
  li > button:first-child { flex: 1; display: flex; align-items: center; gap: 10px; padding: 7px 8px; min-width: 0; }
  .dot { width: 9px; height: 9px; border-radius: 50%; flex: none; }
  .nm { flex: 1; min-width: 0; font-size: 13px; display: flex; flex-direction: column; overflow: hidden; }
  .nm small { font-size: 11px; color: #61738c; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .chip { font-size: 11px; padding: 2px 8px; border-radius: 99px; background: #2a1215; color: #fca5a5; white-space: nowrap; }
  .chip.soft { background: #1b2535; color: #94a3b8; }
  .chip.tag { background: color-mix(in srgb, var(--c) 15%, transparent); color: var(--c); }
  .sz { font-size: 12px; color: var(--muted); min-width: 64px; text-align: right; }
  .rv { width: 28px; height: 28px; color: var(--muted); border-radius: 6px; text-align: center; opacity: 0; }
  li:hover .rv { opacity: 1; }
  .rv:hover { color: var(--accent); background: #1b2535; }
  .empty { padding: 8px; color: var(--muted); font-size: 13px; }
</style>
