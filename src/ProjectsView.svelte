<script module>
  let cached = null; // last report while the window is open
</script>

<script>
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import { fmtBytes, fmtAgo } from './format.js';
  import { privacy, pv } from './privacy.svelte.js';

  let { onspace } = $props();

  let data = $state(cached);
  let loading = $state(false);
  let error = $state('');
  let filter = $state('all');
  let sort = $state('recent');
  let q = $state('');

  async function load() {
    loading = true;
    try { data = cached = await invoke('projects_report'); error = ''; }
    catch (e) { error = String(e); }
    loading = false;
  }

  const lastTouch = (p) => Math.max(p.newest || 0, p.git?.last_commit || 0);
  const attention = (p) => !p.backups.length || (p.git && (p.git.changed + p.git.untracked > 0 || p.git.ahead > 0 || (!p.git.remote && !p.git.upstream)));

  const list = $derived.by(() => {
    let v = data?.projects ?? [];
    if (filter === 'git') v = v.filter((p) => p.git);
    if (filter === 'attention') v = v.filter(attention);
    const s = q.trim().toLowerCase();
    if (s) v = v.filter((p) => (p.name + ' ' + p.path + ' ' + p.tags.join(' ')).toLowerCase().includes(s));
    const by = { recent: (a, b) => lastTouch(b) - lastTouch(a), size: (a, b) => b.size - a.size, name: (a, b) => a.name.localeCompare(b.name) }[sort];
    return [...v].sort(by);
  });
  const stats = $derived.by(() => {
    const ps = data?.projects ?? [];
    return {
      total: ps.length,
      git: ps.filter((p) => p.git).length,
      dirty: ps.filter((p) => p.git && p.git.changed + p.git.untracked > 0).length,
      unpushed: ps.filter((p) => p.git && p.git.ahead > 0).length,
      unbacked: ps.filter((p) => !p.backups.length).length,
      size: ps.reduce((a, p) => a + p.size, 0),
    };
  });

  const run = (cmd, args) => invoke(cmd, args).catch((e) => (error = String(e)));

  onMount(() => { if (!cached) load(); });
</script>

<div class="pv">
  <div class="intro">
    <div>
      <h1>Projects</h1>
      {#if data}
        <p>
          {stats.total} projects · {fmtBytes(stats.size)} · {stats.git} git repos
          {#if stats.dirty}<span class="w"> · {stats.dirty} with uncommitted changes</span>{/if}
          {#if stats.unpushed}<span class="w"> · {stats.unpushed} not pushed</span>{/if}
          {#if stats.unbacked}<span class="bad"> · {stats.unbacked} not backed up</span>{/if}
        </p>
      {:else}
        <p>Finding your projects and asking git about each one…</p>
      {/if}
    </div>
    <div class="tools">
      <input placeholder="Search" bind:value={q} />
      <div class="seg">
        <button class:on={filter === 'all'} onclick={() => (filter = 'all')}>All</button>
        <button class:on={filter === 'git'} onclick={() => (filter = 'git')}>Git</button>
        <button class:on={filter === 'attention'} onclick={() => (filter = 'attention')}>Needs a look</button>
      </div>
      <select bind:value={sort}>
        <option value="recent">Last worked on</option>
        <option value="size">Size</option>
        <option value="name">Name</option>
      </select>
      <button class="re" disabled={loading} onclick={load} title="Check again">{loading ? '…' : '↻'}</button>
    </div>
  </div>
  {#if error}<div class="err">{error}</div>{/if}

  {#if data}
    <div class="grid">
      {#each list as p (p.path)}
        <article class="card">
          <div class="top">
            <h3 class:pvt={privacy.on}>{pv(p.name)}</h3>
            <span class="when" title="Last file change or commit">{fmtAgo(lastTouch(p))}</span>
          </div>
          {#if p.tags.length}
            <div class="tags">{#each p.tags as t (t)}<span>{t}</span>{/each}</div>
          {/if}
          {#if p.description}<p class="desc" class:pvt={privacy.on}>{pv(p.description)}</p>{/if}
          <div class="path" class:pvt={privacy.on}>{pv(p.path)}</div>

          <ul class="facts">
            <li><span>Size</span><b>{fmtBytes(p.size)}</b></li>
            {#if p.git}
              <li>
                <span>Git</span>
                <b>
                  <span class:pvt={privacy.on}>{pv(p.git.branch || 'no branch')}</span>
                  {#if p.git.changed + p.git.untracked > 0}
                    <em class="w">{p.git.changed ? `${p.git.changed} changed` : ''}{p.git.changed && p.git.untracked ? ', ' : ''}{p.git.untracked ? `${p.git.untracked} new` : ''}</em>
                  {:else}
                    <em class="ok">clean</em>
                  {/if}
                  {#if p.git.ahead}<em class="w">↑{p.git.ahead} not pushed</em>{/if}
                  {#if p.git.behind}<em class="info">↓{p.git.behind} behind</em>{/if}
                  {#if !p.git.remote && !p.git.upstream}<em class="mut">local only</em>{/if}
                </b>
              </li>
              {#if p.git.last_commit}
                <li><span>Commit</span><b class="msg"><span class:pvt={privacy.on}>{pv(p.git.last_message)}</span> <i>{fmtAgo(p.git.last_commit)}</i></b></li>
              {/if}
            {:else}
              <li><span>Git</span><b class="mutb">not a git repo</b></li>
            {/if}
            <li>
              <span>Backup</span>
              {#if p.backups.length}
                <b class="okb" class:pvt={privacy.on}>✓ {privacy.on ? pv(p.backups.join(', ')) : p.backups.join(', ')}</b>
              {:else}
                <b class="badb">✗ not backed up</b>
              {/if}
            </li>
          </ul>

          <div class="acts">
            {#if data.editor}<button class="main" onclick={() => run('open_in_editor', { path: p.path })}>Open in {data.editor}</button>{/if}
            <button onclick={() => run('open_path', { path: p.path })}>Folder</button>
            <button onclick={() => run('open_terminal', { path: p.path })}>Terminal</button>
            {#if p.git?.remote}<button onclick={() => run('open_url', { url: p.git.remote })} title={privacy.on ? '' : p.git.remote}>{p.git.remote.includes('github.com') ? 'GitHub' : 'Remote'}</button>{/if}
            {#if p.dir_id}<button onclick={() => onspace?.(p.scan_id, p.dir_id)}>Space</button>{/if}
          </div>
        </article>
      {:else}
        <p class="muted">No projects match.</p>
      {/each}
    </div>
    <p class="foot">Projects are the folders in Documents\Projects (and git repos in your user folder). Sizes and last edits come from the latest scan; git is checked live.</p>
  {/if}
</div>

<style>
  .pv { grid-column: 1 / -1; overflow-y: auto; padding: 22px 28px 40px; min-height: 0; }
  .intro { display: flex; align-items: flex-end; justify-content: space-between; gap: 16px; flex-wrap: wrap; }
  h1 { margin: 0 0 6px; font-size: 22px; }
  .intro p, .muted { margin: 0; color: var(--muted); font-size: 13.5px; }
  .w { color: #fcd34d; }
  .bad { color: #fca5a5; }
  .tools { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .tools input, .tools select { background: var(--panel2); border: 1px solid var(--line); border-radius: 8px; color: var(--text); padding: 6px 10px; font-size: 12.5px; }
  .tools input { width: 150px; }
  .seg { display: flex; gap: 3px; background: var(--panel2); border: 1px solid var(--line); border-radius: 9px; padding: 3px; }
  .seg button { padding: 4px 11px; border-radius: 7px; font-size: 12.5px; color: var(--muted); }
  .seg button.on { background: #12302f; color: var(--accent); }
  .re { color: var(--accent); padding: 5px 9px; border: 1px solid var(--line); border-radius: 8px; }
  .err { margin-top: 12px; padding: 9px 12px; border-radius: 8px; background: #2a1215; border: 1px solid #6b2229; color: #fca5a5; font-size: 13px; }
  .grid { margin-top: 18px; display: grid; grid-template-columns: repeat(auto-fill, minmax(340px, 1fr)); gap: 14px; align-items: start; }
  .card { border: 1px solid var(--line); border-radius: 12px; background: var(--panel2); padding: 14px 16px 12px; display: flex; flex-direction: column; gap: 7px; min-width: 0; }
  .card:hover { border-color: #2c3b52; }
  .top { display: flex; align-items: baseline; gap: 10px; }
  h3 { margin: 0; font-size: 15px; flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .when { font-size: 11.5px; color: var(--muted); white-space: nowrap; }
  .tags { display: flex; gap: 5px; flex-wrap: wrap; }
  .tags span { font-size: 10.5px; padding: 1px 8px; border-radius: 99px; background: #15233a; color: #93c5fd; font-weight: 600; }
  .desc { margin: 0; font-size: 12.5px; color: #c4cfde; line-height: 1.45; display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
  .path { font-size: 11px; color: #6b7d96; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .facts { list-style: none; margin: 2px 0 0; padding: 0; display: flex; flex-direction: column; gap: 4px; border-top: 1px solid #172131; padding-top: 8px; }
  .facts li { display: flex; gap: 10px; font-size: 12.5px; align-items: baseline; min-width: 0; }
  .facts li > span { width: 52px; flex: none; color: var(--muted); font-size: 11.5px; }
  .facts b { font-weight: 500; min-width: 0; display: flex; gap: 8px; flex-wrap: wrap; align-items: baseline; }
  .facts b.msg { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: #c4cfde; }
  .facts i { color: var(--muted); font-style: normal; font-size: 11.5px; }
  em { font-style: normal; font-size: 11.5px; }
  em.ok, .okb { color: #5eead4; }
  em.info { color: #7dd3fc; }
  em.mut, .mutb { color: var(--muted); }
  .badb { color: #fca5a5; }
  .acts { display: flex; gap: 6px; flex-wrap: wrap; margin-top: 4px; }
  .acts button { padding: 5px 10px; border-radius: 7px; border: 1px solid var(--line); color: var(--accent); font-size: 12px; }
  .acts button:hover { border-color: var(--accent); background: #12302f; }
  .acts button.main { background: #12302f; border-color: #1d4a47; }
  .foot { font-size: 12px; color: var(--muted); margin-top: 18px; }
</style>
