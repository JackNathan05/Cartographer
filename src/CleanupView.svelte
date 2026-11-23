<script>
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { onMount } from 'svelte';
  import { fmtBytes, fmtAgo, fmtDate } from './format.js';
  import { privacy, pv } from './privacy.svelte.js';

  let { onspace, onreveal, onchanged } = $props();

  let report = $state(null);
  let error = $state('');
  let filter = $state('all');
  let confirm = $state(null); // suggestion or action awaiting a yes
  // Lives in the app, not this tab, so it survives switching tabs: { running, queue, results }
  let jobs = $state({ running: null, queue: [], results: [] });
  let flash = $state(''); // a one-off problem, e.g. "already in the queue"

  const KIND = {
    project: { label: 'Projects', color: '#38bdf8' },
    game: { label: 'Games', color: '#4ade80' },
    app: { label: 'Apps', color: '#a78bfa' },
    download: { label: 'Downloads', color: '#fbbf24' },
  };
  const ACTION = {
    archive: { verb: 'Archive', note: 'Zip, check every file, then send the originals to the Recycle Bin. Restore any time.' },
    delete: { verb: 'Delete', note: 'Send to the Recycle Bin. Restore any time from here or the Recycle Bin.' },
    uninstall: { verb: 'Uninstall…', note: "Opens Windows' Installed apps page so you can uninstall it there." },
  };

  async function load() {
    try { report = await invoke('cleanup_report'); error = ''; }
    catch (e) { error = String(e); }
  }

  const list = $derived((report?.suggestions ?? []).filter((s) => filter === 'all' || s.kind === filter));
  const counts = $derived(
    (report?.suggestions ?? []).reduce((m, s) => ((m[s.kind] = (m[s.kind] ?? 0) + 1), m), {}),
  );
  const freeable = $derived(list.reduce((a, s) => a + s.size, 0));
  const archived = $derived((report?.actions ?? []).filter((a) => !a.restored));
  const history = $derived((report?.actions ?? []).filter((a) => a.restored));

  async function hide(s, days) {
    await invoke('cleanup_hide', { key: s.key, days });
    await load();
  }

  const job = $derived(jobs.running);
  // path / restore id -> 'running' or its place in the queue (1, 2, ...)
  const jobOf = $derived.by(() => {
    const m = {};
    if (jobs.running) m[jobs.running.path ?? jobs.running.id] = 'running';
    jobs.queue.forEach((q, i) => (m[q.path ?? q.id] = { n: i + 1, job: q.job }));
    return m;
  });

  async function run(op, args, label) {
    confirm = null;
    flash = '';
    try {
      await invoke('cleanup_run', { op, ...args, label });
    } catch (e) {
      flash = String(e);
    }
  }

  const unqueue = (job) => invoke('cleanup_unqueue', { job });
  const dismiss = (r) => invoke('cleanup_ack', { job: r.job });
  const dismissAll = () => jobs.results.forEach(dismiss);

  function doAction(s) {
    if (s.action === 'uninstall') { invoke('open_uninstall'); return; }
    run(s.action, { path: s.path, kind: s.kind }, `${ACTION[s.action].verb} ${s.name}`);
  }

  onMount(() => {
    load();
    const un = [];
    (async () => {
      un.push(await listen('cleanup-jobs', (e) => (jobs = e.payload)));
      un.push(await listen('cleanup-progress', (e) => {
        if (jobs.running?.job === e.payload.job) jobs.running = { ...jobs.running, ...e.payload };
      }));
      un.push(await listen('cleanup-done', async (e) => {
        await load();
        if (e.payload.ok) onchanged?.();
      }));
      jobs = await invoke('cleanup_jobs');
    })();
    return () => un.forEach((f) => f());
  });
</script>

<div class="cv">
  <div class="intro">
    <h1>Cleanup</h1>
    {#if report}
      <p>
        {#if report.suggestions.length}
          <b>{report.suggestions.length}</b> things you haven't used in a while, <b>{fmtBytes(report.suggestions.reduce((a, s) => a + s.size, 0))}</b> in total.
          Everything here can be undone.
        {:else}
          Nothing to suggest right now. Everything you have was used recently.
        {/if}
      </p>
    {/if}
  </div>

  {#if error}<div class="err">{error}</div>{/if}

  {#if job}
    <div class="job">
      <div class="spinner"></div>
      <div class="jt">
        <b class:pvt={privacy.on}>{job.label}</b>
        <span>{job.phase}{job.total ? ` · ${fmtBytes(job.done)} of ${fmtBytes(job.total)}` : ''}</span>
        {#if job.total}<div class="jbar"><div style="width:{Math.min(100, (job.done / job.total) * 100)}%"></div></div>{/if}
        {#if jobs.queue.length}
          <div class="queue">
            <span class="qh">Up next · starts by itself when the one before finishes. Your PC won't go to sleep until the queue is done.</span>
            <ol>
              {#each jobs.queue as q (q.job)}
                <li><span class:pvt={privacy.on}>{q.label}</span><button class="link" onclick={() => unqueue(q.job)}>Remove</button></li>
              {/each}
            </ol>
          </div>
        {/if}
      </div>
    </div>
  {/if}
  {#if flash}
    <div class="toast bad"><span>!</span>{flash}<button onclick={() => (flash = '')}>✕</button></div>
  {/if}
  {#each jobs.results as r (r.job)}
    <div class="toast" class:bad={!r.ok}>
      <span>{r.ok ? '✓' : '!'}</span><span class="msg" class:pvt={privacy.on}>{r.message}</span>
      <button onclick={() => dismiss(r)}>✕</button>
    </div>
  {/each}
  {#if jobs.results.length > 1}
    <button class="link dismiss-all" onclick={dismissAll}>Dismiss all {jobs.results.length}</button>
  {/if}

  {#if report}
    <section>
      <div class="filters">
        <button class:on={filter === 'all'} onclick={() => (filter = 'all')}>All <em>{report.suggestions.length}</em></button>
        {#each Object.entries(KIND) as [k, v] (k)}
          {#if counts[k]}
            <button class:on={filter === k} onclick={() => (filter = k)}><span class="dot" style="background:{v.color}"></span>{v.label} <em>{counts[k]}</em></button>
          {/if}
        {/each}
        <span class="sum">{fmtBytes(freeable)} could be freed</span>
      </div>

      <ul class="sugs">
        {#each list as s (s.key)}
          <li>
            <span class="dot big" style="background:{KIND[s.kind]?.color}"></span>
            <div class="main">
              <div class="name">
                <button class="link" class:pvt={privacy.on} onclick={() => s.scan_id && onspace?.(s.scan_id, s.dir_id)} disabled={!s.scan_id}>{pv(s.name)}</button>
                <span class="size">{fmtBytes(s.size)}</span>
              </div>
              <div class="reason">{s.reason}</div>
              <div class="meta">
                Last used {fmtDate(s.last_used)} <span class="how">({s.last_used_how})</span>
                {#if s.backed_up.length}<span class="chip ok">✓ backed up · {s.backed_up.join(' · ')}</span>
                {:else if s.kind === 'project' || s.kind === 'download'}<span class="chip warn">not in a backup</span>{/if}
              </div>
              <div class="path" class:pvt={privacy.on}>{pv(s.path)}</div>
            </div>
            <div class="btns">
              {#if jobOf[s.path] === 'running'}
                <button class="primary" disabled>Working…</button>
              {:else if jobOf[s.path]}
                <button class="primary queued" disabled>Queued #{jobOf[s.path].n}</button>
              {:else if job && s.action !== 'uninstall'}
                <button class="primary" title="Starts by itself when the current one finishes" onclick={() => doAction(s)}>Queue</button>
              {:else}
                <button class="primary" onclick={() => (s.action === 'uninstall' ? doAction(s) : (confirm = s))}>{ACTION[s.action].verb}</button>
              {/if}
              <div class="small">
                {#if jobOf[s.path] && jobOf[s.path] !== 'running'}<button onclick={() => unqueue(jobOf[s.path].job)}>Remove from queue</button>{/if}
                <button onclick={() => hide(s, 30)}>Snooze 30 days</button>
                <button onclick={() => hide(s, null)}>Never suggest</button>
                <button onclick={() => onreveal?.(s.path)}>Show</button>
              </div>
            </div>
          </li>
        {:else}
          <li class="empty">Nothing in this group.</li>
        {/each}
      </ul>
      {#if report.hidden}
        <p class="note">{report.hidden} snoozed or hidden · <button class="link" onclick={async () => { await invoke('cleanup_unhide_all'); load(); }}>show them again</button></p>
      {/if}
    </section>

    <section>
      <h2>Archived and deleted</h2>
      <p class="note">Archives go to <code>{pv(report.archive_dir)}</code>{report.archive_backed_up.length ? `, which is backed up by ${report.archive_backed_up.join(' and ')}` : ' (not in any backup)'}.</p>
      <ul class="acts">
        {#each archived as a (a.id)}
          <li>
            <span class="tag {a.kind}">{a.kind === 'archive' ? 'Archived' : 'Deleted'}</span>
            <div class="main">
              <div class="name"><b class:pvt={privacy.on}>{pv(a.name)}</b><span class="size">{fmtBytes(a.bytes || a.zip_bytes)}{a.kind === 'archive' ? ` → ${fmtBytes(a.zip_bytes)} zip` : ''}</span></div>
              <div class="meta">{fmtDate(a.time)} · {fmtAgo(a.time)}{a.kind === 'archive' ? ` · ${a.files} files, verified` : ''}</div>
              <div class="path" class:pvt={privacy.on}>from {pv(a.from)}{a.to ? ` · zip: ${pv(a.to)}` : ''}</div>
            </div>
            <div class="btns">
              {#if jobOf[a.id] === 'running'}
                <button class="primary" disabled>Restoring…</button>
              {:else if jobOf[a.id]}
                <button class="primary queued" disabled>Queued #{jobOf[a.id].n}</button>
              {:else if job}
                <button class="primary" title="Starts by itself when the current one finishes" onclick={() => run('restore', { id: a.id }, `Restore ${a.name}`)}>Queue restore</button>
              {:else}
                <button class="primary" onclick={() => (confirm = { restore: a })}>Restore</button>
              {/if}
              {#if a.to}<div class="small"><button onclick={() => onreveal?.(a.to)}>Show zip</button></div>{/if}
            </div>
          </li>
        {:else}
          <li class="empty">Nothing archived or deleted yet.</li>
        {/each}
      </ul>
      {#if history.length}
        <details>
          <summary>History ({history.length} restored)</summary>
          <ul class="hist">
            {#each history as a (a.id)}
              <li>{fmtDate(a.time)} · {a.kind === 'archive' ? 'Archived' : 'Deleted'} <b>{pv(a.name)}</b> · restored {fmtAgo(a.restored)}</li>
            {/each}
          </ul>
        </details>
      {/if}
    </section>
  {/if}
</div>

{#if confirm}
  <div class="modal" role="presentation" onclick={(e) => e.target === e.currentTarget && (confirm = null)}>
    <div class="box">
      {#if confirm.restore}
        {@const a = confirm.restore}
        <h3>Restore {pv(a.name)}?</h3>
        <p>{a.kind === 'archive' ? `Unzips it back to ${pv(a.from)}, checks every file came back, then sends the zip to the Recycle Bin.` : `Takes it back out of the Recycle Bin to ${pv(a.from)}.`}</p>
        <div class="row">
          <button class="primary" onclick={() => run('restore', { id: a.id }, `Restore ${a.name}`)}>Restore</button>
          <button onclick={() => (confirm = null)}>Cancel</button>
        </div>
      {:else}
        {@const s = confirm}
        <h3>{ACTION[s.action].verb} {pv(s.name)}?</h3>
        <dl>
          <dt>Size</dt><dd>{fmtBytes(s.size)}</dd>
          <dt>Location</dt><dd class="path">{pv(s.path)}</dd>
          {#if s.action === 'archive'}<dt>Zip goes to</dt><dd class="path">{pv(report.archive_dir)}\{s.kind === 'project' ? 'Projects' : 'Downloads'}</dd>{/if}
        </dl>
        <p>{ACTION[s.action].note}</p>
        <div class="row">
          <button class="primary" onclick={() => doAction(s)}>{ACTION[s.action].verb}</button>
          <button onclick={() => (confirm = null)}>Cancel</button>
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .cv { grid-column: 1 / -1; overflow-y: auto; padding: 22px 28px 40px; min-height: 0; }
  .intro h1 { margin: 0 0 6px; font-size: 22px; }
  .intro p { margin: 0; color: #b8c4d6; font-size: 14px; }
  section { margin-top: 22px; }
  h2 { font-size: 11.5px; text-transform: uppercase; letter-spacing: 1.3px; color: var(--muted); margin: 0 0 8px; }
  code { font-size: 12px; color: #8aa0bd; }
  .err { margin-top: 12px; padding: 9px 12px; border-radius: 8px; background: #2a1215; border: 1px solid #6b2229; color: #fca5a5; font-size: 13px; }
  .job { margin-top: 14px; display: flex; gap: 12px; align-items: flex-start; padding: 12px 14px; border-radius: 10px; background: #0f1f26; border: 1px solid #1d4a47; }
  .jt { flex: 1; display: flex; flex-direction: column; gap: 3px; font-size: 13px; }
  .jt span { color: var(--muted); font-size: 12px; }
  .jbar { height: 5px; border-radius: 3px; background: #16303a; overflow: hidden; margin-top: 3px; }
  .jbar div { height: 100%; background: #2dd4bf; transition: width 0.2s; }
  .spinner { width: 18px; height: 18px; border-radius: 50%; border: 2px solid #1d4a47; border-top-color: #5eead4; animation: spin 0.8s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  .toast { margin-top: 14px; display: flex; gap: 10px; align-items: center; padding: 10px 14px; border-radius: 10px; background: #0f1f26; border: 1px solid #1d4a47; color: #ccfbf1; font-size: 13px; }
  .toast + .toast { margin-top: 6px; }
  .toast .msg { overflow-wrap: anywhere; }
  .dismiss-all { margin-top: 6px; font-size: 12px; }
  .queue { margin-top: 8px; font-size: 12px; }
  .queue .qh { color: var(--muted); }
  .queue ol { margin: 6px 0 0; padding-left: 18px; display: grid; gap: 3px; }
  .queue li { color: #ccfbf1; }
  .queue li button { margin-left: 10px; font-size: 12px; }
  .primary.queued { opacity: 0.75; }
  .toast.bad { background: #2a1215; border-color: #6b2229; color: #fecaca; }
  .toast button { margin-left: auto; color: var(--muted); }
  .filters { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; margin-bottom: 10px; }
  .filters button { display: inline-flex; align-items: center; gap: 6px; padding: 5px 11px; border-radius: 99px; border: 1px solid var(--line); font-size: 12.5px; color: #b8c4d6; }
  .filters button.on { border-color: #2dd4bf; color: var(--text); background: #0f1f26; }
  .filters em { font-style: normal; color: var(--muted); font-size: 11.5px; }
  .filters .sum { margin-left: auto; color: var(--muted); font-size: 12.5px; }
  .dot { width: 9px; height: 9px; border-radius: 50%; display: inline-block; flex: none; }
  .dot.big { width: 11px; height: 11px; margin-top: 5px; }
  ul { list-style: none; margin: 0; padding: 0; }
  .sugs li, .acts li { display: flex; gap: 14px; align-items: flex-start; padding: 12px 14px; border: 1px solid var(--line); border-radius: 10px; margin-bottom: 8px; background: var(--panel2); }
  .main { flex: 1; min-width: 0; }
  .name { display: flex; align-items: baseline; gap: 10px; }
  .name .link, .name b { font-size: 14.5px; font-weight: 600; }
  .link { color: var(--text); }
  .link:not(:disabled):hover { color: var(--accent); text-decoration: underline; }
  .size { color: var(--muted); font-size: 12.5px; }
  .reason { margin-top: 3px; font-size: 13px; color: #e2e8f0; }
  .meta { margin-top: 4px; font-size: 12px; color: var(--muted); display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
  .how { color: #61738c; }
  .path { margin-top: 3px; font-size: 11.5px; color: #61738c; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .chip { font-size: 11px; padding: 1px 8px; border-radius: 99px; }
  .chip.ok { background: #0f2a22; color: #5eead4; }
  .chip.warn { background: #2a1215; color: #fca5a5; }
  .btns { display: flex; flex-direction: column; align-items: flex-end; gap: 6px; }
  .btns .primary, .row .primary { padding: 7px 14px; border-radius: 8px; background: #134e4a; border: 1px solid #2dd4bf; color: #ccfbf1; font-size: 12.5px; }
  .btns .primary:disabled { opacity: 0.45; }
  .btns .small { display: flex; gap: 4px; }
  .btns .small button { font-size: 11.5px; color: var(--muted); padding: 3px 7px; border-radius: 6px; }
  .btns .small button:hover { color: var(--text); background: var(--panel); }
  .tag { font-size: 11px; padding: 3px 9px; border-radius: 99px; margin-top: 2px; white-space: nowrap; }
  .tag.archive { background: #0f2a22; color: #5eead4; }
  .tag.delete { background: #2a2108; color: #fcd34d; }
  .empty { padding: 10px; color: var(--muted); font-size: 13px; border: 1px dashed var(--line) !important; background: none !important; }
  .note { font-size: 12px; color: var(--muted); margin: 6px 0 10px; }
  .note .link { color: var(--accent); font-size: 12px; }
  details { margin-top: 8px; font-size: 12.5px; color: var(--muted); }
  .hist li { padding: 3px 0; }
  .modal { position: fixed; inset: 0; background: rgba(3,6,12,0.6); display: grid; place-items: center; z-index: 10; }
  .box { width: min(480px, 92vw); background: #0f1622; border: 1px solid #22304a; border-radius: 14px; padding: 20px 22px; box-shadow: 0 20px 60px rgba(0,0,0,0.5); }
  .box h3 { margin: 0 0 10px; font-size: 17px; }
  .box p { font-size: 13px; color: #b8c4d6; line-height: 1.5; }
  .box dl { display: grid; grid-template-columns: auto 1fr; gap: 5px 12px; font-size: 12.5px; margin: 0 0 8px; }
  .box dt { color: var(--muted); }
  .box dd { margin: 0; word-break: break-all; }
  .row { display: flex; gap: 8px; margin-top: 14px; }
  .row button:not(.primary) { padding: 7px 14px; border-radius: 8px; border: 1px solid var(--line); font-size: 12.5px; color: var(--muted); }
</style>
