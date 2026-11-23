<script module>
  // keep the last report per period while the window is open, so switching tabs is instant
  const cache = new Map();
</script>

<script>
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import { fmtBytes, fmtAgo, fmtDate } from './format.js';
  import { privacy, pv } from './privacy.svelte.js';

  let { onspace, onreveal } = $props();

  const PERIODS = [
    { days: 1, label: 'Today' },
    { days: 7, label: '7 days' },
    { days: 30, label: '30 days' },
    { days: 0, label: 'All time' },
  ];
  let days = $state(7);
  let data = $state(cache.get(7) ?? null);
  let loading = $state(false);
  let error = $state('');

  async function load(d = days, force = false) {
    days = d;
    if (!force && cache.has(d)) { data = cache.get(d); return; }
    loading = true;
    try {
      const r = await invoke('timeline', { days: d });
      cache.set(d, r);
      if (days === d) data = r;
      error = '';
    } catch (e) { error = String(e); }
    loading = false;
  }

  const compared = $derived((data?.drives ?? []).filter((d) => d.since));
  const sinceText = $derived.by(() => {
    if (!data) return '';
    if (!compared.length) return '';
    const oldest = Math.min(...compared.map((d) => d.since));
    if (data.days && oldest > data.cutoff + 3600) return `Compared with ${fmtDate(oldest)}, the oldest history so far.`;
    return `Compared with ${fmtDate(oldest)}.`;
  });

  const KIND = { new: 'New', grew: 'Grew', shrank: 'Shrank', removed: 'Removed' };
  const signed = (b) => (b > 0 ? '+' : b < 0 ? '−' : '') + fmtBytes(Math.abs(b));

  function spark(points) {
    if (!points || points.length < 2) return '';
    const xs = points.map((p) => p.time), ys = points.map((p) => p.bytes);
    const [x0, x1] = [Math.min(...xs), Math.max(...xs)];
    const [y0, y1] = [Math.min(...ys), Math.max(...ys)];
    return points
      .map((p) => `${(((p.time - x0) / (x1 - x0 || 1)) * 116 + 2).toFixed(1)},${(30 - ((p.bytes - y0) / (y1 - y0 || 1)) * 26).toFixed(1)}`)
      .join(' ');
  }

  onMount(() => load(days, !cache.has(days)));
</script>

<div class="tv">
  <div class="intro">
    <div>
      <h1>Timeline</h1>
      <p>What changed on your drives{data?.days ? ` in the last ${data.days === 1 ? 'day' : data.days + ' days'}` : ''}.</p>
    </div>
    <div class="periods">
      {#each PERIODS as p (p.days)}
        <button class:on={days === p.days} onclick={() => load(p.days)}>{p.label}</button>
      {/each}
      <button class="re" disabled={loading} onclick={() => load(days, true)} title="Check again">{loading ? '…' : '↻'}</button>
    </div>
  </div>
  {#if error}<div class="err">{error}</div>{/if}

  {#if !data}
    <p class="muted">Reading your history…</p>
  {:else}
    <section>
      <h2>Drives</h2>
      <div class="drives">
        {#each data.drives as d (d.root)}
          {@const last = d.points[d.points.length - 1]}
          <div class="dcard">
            <div class="dtop">
              <b>{d.root.slice(0, 2)}</b>
              <span>{fmtBytes(last?.bytes)}</span>
              {#if d.since}
                <span class="chg" class:up={d.change > 0} class:down={d.change < 0}>{signed(d.change)}</span>
              {/if}
            </div>
            {#if d.points.length > 1}
              <svg viewBox="0 0 120 32" preserveAspectRatio="none"><polyline points={spark(d.points)} /></svg>
              <span class="dsub">{d.points.length} snapshots since {fmtDate(d.points[0].time)}</span>
            {:else}
              <div class="flat"></div>
              <span class="dsub">History started {fmtDate(last?.time)}</span>
            {/if}
          </div>
        {/each}
      </div>
    </section>

    <section>
      <h2>What grew and shrank <span class="note">{sinceText}</span></h2>
      {#if !compared.length}
        <div class="blank">
          Cartographer started keeping history {data.history_start ? fmtDate(data.history_start) : 'today'}. Each scan leaves a small
          daily snapshot of your folders, and the map updates itself in the background, so
          <b>come back tomorrow</b> to see what grew, shrank, appeared or was deleted.
        </div>
      {:else if !data.changes.length}
        <div class="blank">No folder changed by 100 MB or more.</div>
      {:else}
        <ul class="rows">
          {#each data.changes as c (c.path)}
            <li>
              <span class="kind {c.kind}">{KIND[c.kind]}</span>
              <div class="txt">
                <b class:pvt={privacy.on}>{pv(c.label)}</b>
                <span class:pvt={privacy.on}>{pv(c.path)}</span>
              </div>
              <div class="num">
                <b class:up={c.delta > 0} class:down={c.delta < 0}>{signed(c.delta)}</b>
                <span>{c.kind === 'new' ? 'now ' + fmtBytes(c.after) : c.kind === 'removed' ? 'was ' + fmtBytes(c.before) : `${fmtBytes(c.before)} → ${fmtBytes(c.after)}`}</span>
              </div>
              <div class="acts">
                {#if c.dir_id}<button onclick={() => onspace?.(c.scan_id, c.dir_id)}>Space</button>{/if}
                {#if c.kind !== 'removed'}<button onclick={() => onreveal?.(c.path)} title="Show in Explorer">↗</button>{/if}
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <div class="two">
      <section>
        <h2>Installed or updated <span class="note">{data.installs.length}</span></h2>
        {#if !data.installs.length}
          <div class="blank">Nothing installed{data.days ? ' in this period' : ''}.</div>
        {:else}
          <ul class="rows compact">
            {#each data.installs as i (i.name + i.when)}
              <li>
                <span class="ico">{i.kind === 'game' ? '🎮' : '📦'}</span>
                <div class="txt">
                  <b class:pvt={privacy.on}>{pv(i.name)}</b>
                  <span class:pvt={privacy.on}>{pv(i.detail || (i.kind === 'game' ? 'Game' : 'App'))}</span>
                </div>
                <div class="num">
                  <b class="plain">{fmtDate(i.when)}</b>
                  <span>{i.size ? fmtBytes(i.size) : fmtAgo(i.when)}</span>
                </div>
                <div class="acts">
                  {#if i.path}<button onclick={() => onreveal?.(i.path)} title="Show in Explorer">↗</button>{/if}
                </div>
              </li>
            {/each}
          </ul>
        {/if}
      </section>

      <section>
        <h2>Big files added or changed <span class="note">50 MB or more</span></h2>
        {#if !data.files.length}
          <div class="blank">No big files{data.days ? ' in this period' : ''}.</div>
        {:else}
          <ul class="rows compact">
            {#each data.files as f (f.dir + f.name)}
              <li>
                <span class="ico">📄</span>
                <div class="txt">
                  <b class:pvt={privacy.on}>{pv(f.name)}</b>
                  <span class:pvt={privacy.on}>{pv(f.dir)}</span>
                </div>
                <div class="num">
                  <b class="plain">{fmtBytes(f.size)}</b>
                  <span>{fmtAgo(f.modified)}</span>
                </div>
                <div class="acts">
                  <button onclick={() => onspace?.(f.scan_id, f.dir_id)}>Space</button>
                  <button onclick={() => onreveal?.(f.dir + '\\' + f.name)} title="Show in Explorer">↗</button>
                </div>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    </div>
    <p class="foot">Folder changes come from daily snapshots (folders of 10 MB or more). Install dates come from Windows and your game launchers; an update can count as a new install date.</p>
  {/if}
</div>

<style>
  .tv { grid-column: 1 / -1; overflow-y: auto; padding: 22px 28px 40px; min-height: 0; }
  .intro { display: flex; align-items: flex-end; justify-content: space-between; gap: 16px; flex-wrap: wrap; }
  h1 { margin: 0 0 6px; font-size: 22px; }
  .intro p, .muted { margin: 0; color: var(--muted); font-size: 13.5px; }
  .periods { display: flex; gap: 4px; background: var(--panel2); border: 1px solid var(--line); border-radius: 9px; padding: 3px; }
  .periods button { padding: 5px 12px; border-radius: 7px; font-size: 12.5px; color: var(--muted); }
  .periods button.on { background: #12302f; color: var(--accent); }
  .periods .re { color: var(--accent); }
  .err { margin-top: 12px; padding: 9px 12px; border-radius: 8px; background: #2a1215; border: 1px solid #6b2229; color: #fca5a5; font-size: 13px; }
  section { margin-top: 22px; min-width: 0; }
  h2 { font-size: 11.5px; text-transform: uppercase; letter-spacing: 1.1px; color: var(--muted); margin: 0 0 9px; font-weight: 600; }
  .note { text-transform: none; letter-spacing: 0; font-weight: 400; margin-left: 6px; color: #6b7d96; }
  .drives { display: grid; grid-template-columns: repeat(auto-fill, minmax(190px, 1fr)); gap: 10px; }
  .dcard { border: 1px solid var(--line); border-radius: 11px; background: var(--panel2); padding: 11px 13px 9px; display: flex; flex-direction: column; gap: 5px; }
  .dtop { display: flex; align-items: baseline; gap: 8px; font-size: 13px; }
  .dtop b { font-size: 15px; }
  .dtop span { color: #c4cfde; }
  .chg { margin-left: auto; font-weight: 600; font-size: 12.5px; }
  svg { width: 100%; height: 32px; }
  polyline { fill: none; stroke: var(--accent); stroke-width: 1.6; vector-effect: non-scaling-stroke; }
  .flat { height: 32px; border-bottom: 1px dashed #26344a; margin-bottom: 0; }
  .dsub { font-size: 11px; color: var(--muted); }
  .up { color: #fbbf24; }
  .down { color: #5eead4; }
  .blank { border: 1px dashed #26344a; border-radius: 10px; padding: 14px 16px; color: var(--muted); font-size: 13px; line-height: 1.55; }
  .blank b { color: var(--text); }
  .rows { list-style: none; margin: 0; padding: 0; border: 1px solid var(--line); border-radius: 11px; background: var(--panel2); }
  .rows li { display: flex; align-items: center; gap: 12px; padding: 9px 13px; border-top: 1px solid #172131; }
  .rows li:first-child { border-top: 0; }
  .rows.compact li { padding: 7px 12px; gap: 10px; }
  .kind { flex: none; width: 66px; text-align: center; font-size: 11px; font-weight: 600; padding: 2px 0; border-radius: 99px; }
  .kind.new { background: #102a3a; color: #7dd3fc; }
  .kind.grew { background: #2a2108; color: #fcd34d; }
  .kind.shrank { background: #0f2a22; color: #5eead4; }
  .kind.removed { background: #24182e; color: #d8b4fe; }
  .ico { flex: none; width: 20px; text-align: center; font-size: 14px; }
  .txt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .txt b { font-size: 13px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .txt span { font-size: 11.5px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .num { flex: none; display: flex; flex-direction: column; align-items: flex-end; gap: 2px; min-width: 96px; }
  .num b { font-size: 13px; }
  .num b.plain { color: var(--text); font-weight: 600; }
  .num span { font-size: 11px; color: var(--muted); white-space: nowrap; }
  .acts { flex: none; display: flex; gap: 5px; }
  .acts button { padding: 4px 9px; border-radius: 7px; border: 1px solid var(--line); color: var(--accent); font-size: 12px; }
  .acts button:hover { border-color: var(--accent); background: #12302f; }
  .two { display: grid; grid-template-columns: repeat(auto-fit, minmax(440px, 1fr)); gap: 0 16px; }
  .foot { font-size: 12px; color: var(--muted); margin-top: 18px; }
</style>
