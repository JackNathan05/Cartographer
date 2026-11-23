<script>
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import { fmtAgo } from './format.js';
  import { privacy, pv } from './privacy.svelte.js';

  let { ongo, onscan, oncount } = $props();

  let report = $state(null);
  let error = $state('');
  let loading = $state(true);

  const AREAS = [
    { key: 'Backups', icon: '🛡' },
    { key: 'Storage', icon: '💾' },
    { key: 'Drives', icon: '🩺' },
    { key: 'Clutter', icon: '🧹' },
    { key: 'Map', icon: '🗺' },
  ];
  const RANK = { bad: 0, warn: 1, info: 2, ok: 3 };

  async function load() {
    loading = true;
    try {
      report = await invoke('health_report');
      error = '';
      oncount?.(issues.length);
    } catch (e) { error = String(e); }
    loading = false;
  }

  const issues = $derived((report?.checks ?? []).filter((c) => c.status === 'bad' || c.status === 'warn'));
  const bad = $derived(issues.filter((c) => c.status === 'bad').length);
  const byArea = $derived(
    AREAS.map((a) => ({ ...a, checks: (report?.checks ?? []).filter((c) => c.area === a.key).sort((x, y) => RANK[x.status] - RANK[y.status]) })),
  );
  const worst = (checks) => checks.reduce((w, c) => (RANK[c.status] < RANK[w] ? c.status : w), 'ok');

  const ACTION_LABEL = { backups: 'Open Backups', cleanup: 'Open Cleanup', space: 'Open in Space', rescan: 'Rescan', open: 'Open folder' };
  function act(c) {
    if (c.action === 'backups' || c.action === 'cleanup') ongo?.(c.action);
    else if (c.action === 'space') ongo?.('space', c.target);
    else if (c.action === 'rescan') onscan?.(c.target);
    else if (c.action === 'open') invoke('open_path', { path: c.target });
  }

  onMount(load);
</script>

<div class="hv">
  <div class="intro">
    <h1>Health</h1>
    {#if report}
      <p class:good={!issues.length}>
        {#if !issues.length}
          ✓ Everything looks good.
        {:else}
          {issues.length} thing{issues.length === 1 ? '' : 's'} need{issues.length === 1 ? 's' : ''} attention{bad ? `, ${bad} important` : ''}.
        {/if}
        <span class="when">Checked {fmtAgo(report.generated)} · <button class="link" onclick={load} disabled={loading}>{loading ? 'checking…' : 'check again'}</button></span>
      </p>
    {:else if loading}
      <p>Checking your PC…</p>
    {/if}
  </div>
  {#if error}<div class="err">{error}</div>{/if}

  {#if report}
    <div class="grid">
      {#each byArea as a (a.key)}
        {#if a.checks.length}
          <section class="card {worst(a.checks)}">
            <h2><span class="ic">{a.icon}</span>{a.key}<span class="pill {worst(a.checks)}">{worst(a.checks) === 'ok' || worst(a.checks) === 'info' ? 'OK' : worst(a.checks) === 'warn' ? 'Check' : 'Fix'}</span></h2>
            <ul>
              {#each a.checks as c, i (i)}
                <li>
                  <span class="st {c.status}"></span>
                  <div class="txt">
                    <b class:pvt={privacy.on && a.key === 'Backups' && c.title.includes('folders')}>{c.title}</b>
                    {#if c.detail}<span class:pvt={privacy.on && (a.key === 'Backups' || c.action === 'open')}>{privacy.on && a.key === 'Backups' ? pv(c.detail) : c.detail}</span>{/if}
                  </div>
                  {#if c.action && ACTION_LABEL[c.action]}
                    <button class="go" onclick={() => act(c)}>{ACTION_LABEL[c.action]}</button>
                  {/if}
                </li>
              {/each}
            </ul>
          </section>
        {/if}
      {/each}
    </div>
    <p class="foot">Green is fine, yellow is worth a look, red needs fixing. Cartographer only reports here; it never changes anything on its own.</p>
  {/if}
</div>

<style>
  .hv { grid-column: 1 / -1; overflow-y: auto; padding: 22px 28px 40px; min-height: 0; }
  .intro h1 { margin: 0 0 6px; font-size: 22px; }
  .intro p { margin: 0; color: #fcd34d; font-size: 14px; }
  .intro p.good { color: #5eead4; }
  .when { color: var(--muted); font-size: 12.5px; margin-left: 10px; }
  .link { color: var(--accent); font-size: 12.5px; }
  .err { margin-top: 12px; padding: 9px 12px; border-radius: 8px; background: #2a1215; border: 1px solid #6b2229; color: #fca5a5; font-size: 13px; }
  .grid { margin-top: 20px; display: grid; grid-template-columns: repeat(auto-fill, minmax(420px, 1fr)); gap: 14px; align-items: start; }
  .card { border: 1px solid var(--line); border-radius: 12px; background: var(--panel2); padding: 14px 16px 8px; border-top: 3px solid #2dd4bf; }
  .card.warn { border-top-color: #fbbf24; }
  .card.bad { border-top-color: #f87171; }
  .card.info { border-top-color: #38bdf8; }
  h2 { display: flex; align-items: center; gap: 9px; margin: 0 0 8px; font-size: 14.5px; }
  .ic { font-size: 15px; }
  .pill { margin-left: auto; font-size: 11px; padding: 2px 9px; border-radius: 99px; font-weight: 600; }
  .pill.ok, .pill.info { background: #0f2a22; color: #5eead4; }
  .pill.warn { background: #2a2108; color: #fcd34d; }
  .pill.bad { background: #2a1215; color: #fca5a5; }
  ul { list-style: none; margin: 0; padding: 0; }
  li { display: flex; align-items: center; gap: 11px; padding: 8px 0; border-top: 1px solid #172131; }
  li:first-child { border-top: 0; }
  .st { width: 10px; height: 10px; border-radius: 50%; flex: none; }
  .st.ok { background: #22c55e; }
  .st.warn { background: #fbbf24; }
  .st.bad { background: #f87171; box-shadow: 0 0 0 3px rgba(248,113,113,0.2); }
  .st.info { background: #38bdf8; }
  .txt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .txt b { font-size: 13px; font-weight: 600; }
  .txt span { font-size: 12px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .go { flex: none; padding: 5px 10px; border-radius: 7px; border: 1px solid var(--line); color: var(--accent); font-size: 12px; }
  .go:hover { border-color: var(--accent); background: #12302f; }
  .foot { font-size: 12px; color: var(--muted); margin-top: 18px; }
</style>
