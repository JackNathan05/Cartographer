<script module>
  // the conversation survives switching tabs (the window keeps it until you start a new one)
  let saved = [];
</script>

<script>
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { onMount, tick } from 'svelte';
  import { privacy, pv } from './privacy.svelte.js';

  let { settings, onenable, onbackend } = $props();

  let status = $state(null);
  let msgs = $state(saved);
  let q = $state('');
  let busy = $state(false);
  let live = $state([]); // tools used while answering
  let error = $state('');
  let keyInput = $state('');
  let showSetup = $state(false);
  let scroller;

  const SUGGEST = [
    'What can I delete or archive to free up 50 GB?',
    'Which projects have work that isn\'t pushed to GitHub?',
    'What changed on my PC this week?',
    'Is everything important backed up?',
    'What are the biggest games and when did I last play them?',
  ];
  const TOOL_LABEL = {
    overview: 'Looked at the overview', find: 'Searched for', folder: 'Opened', health: 'Ran health checks',
    projects: 'Read project cards', apps_and_games: 'Listed apps and games', cleanup_suggestions: 'Read cleanup suggestions',
    backups: 'Checked backups', changes: 'Checked recent changes',
  };
  const stepText = (s) => {
    const base = TOOL_LABEL[s.tool] ?? s.tool;
    if (s.tool === 'find') return `${base} “${s.input?.query ?? ''}”`;
    if (s.tool === 'folder') return `${base} ${s.input?.path ?? ''}`;
    if (s.tool === 'changes') return `${base} (${s.input?.days ?? 7} days)`;
    if (s.tool === 'apps_and_games' && s.input?.query) return `${base} matching “${s.input.query}”`;
    return base;
  };

  const usingNow = $derived.by(() => {
    if (!status) return '';
    const b = settings?.ask_backend ?? 'auto';
    if (b === 'claude_code' || (b === 'auto' && status.claude_code)) return status.claude_code ? 'Claude Code (your Claude login)' : 'Claude Code (not installed)';
    return status.api_key ? `Anthropic API (${status.model})` : 'Anthropic API (no key yet)';
  });
  const ready = $derived(!!status && (((settings?.ask_backend ?? 'auto') !== 'api' && status.claude_code) || (((settings?.ask_backend ?? 'auto') !== 'claude_code') && !!status.api_key)));

  async function refreshStatus() {
    try { status = await invoke('ask_status'); } catch (e) { error = String(e); }
  }

  async function saveKey(clear = false) {
    error = '';
    try { status = await invoke('ask_set_key', { key: clear ? '' : keyInput }); keyInput = ''; }
    catch (e) { error = String(e); }
  }

  async function send(text = q) {
    const question = text.trim();
    if (!question || busy) return;
    error = '';
    q = '';
    msgs = saved = [...msgs, { role: 'user', text: question }];
    busy = true;
    live = [];
    await scrollDown();
    try {
      const a = await invoke('ask_question', { question });
      msgs = saved = [...msgs, { role: 'claude', ...a }];
    } catch (e) {
      msgs = saved = [...msgs, { role: 'error', text: String(e) }];
    }
    busy = false;
    await scrollDown();
  }

  async function reset() {
    await invoke('ask_new');
    msgs = saved = [];
  }

  async function scrollDown() {
    await tick();
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
  }

  // --- tiny, safe markdown: escape first, then a few patterns
  const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  // in plain text a path ends at the first space (paths with spaces are linked when Claude puts them in `code`)
  const PATH = /\b[A-Z]:\\[^\s<>"|*?`]*/g;
  const pathLink = (p) => `<a class="path" data-path="${p}" title="Show in Explorer">${p}</a>`;
  // split on `code` spans; paths become links in both, **bold** only outside code
  function inline(s) {
    return s
      .split(/(`[^`]+`)/)
      .map((part) => {
        if (part.startsWith('`') && part.endsWith('`') && part.length > 1) {
          const c = esc(part.slice(1, -1));
          // a whole code span that is a path (spaces allowed) is one link
          return /^[A-Z]:\\/.test(c) ? `<code>${pathLink(c)}</code>` : `<code>${c}</code>`;
        }
        return linkPaths(esc(part)).replace(/\*\*([^*]+)\*\*/g, '<b>$1</b>');
      })
      .join('');
  }
  function linkPaths(h) {
    return h.replace(PATH, (p) => {
      const clean = p.replace(/[.,;:)\]]+$/, '');
      return pathLink(clean) + p.slice(clean.length);
    });
  }
  function md(text) {
    const lines = (text ?? '').split(/\r?\n/);
    let out = '', list = null;
    const close = () => { if (list) { out += `</${list}>`; list = null; } };
    for (const raw of lines) {
      const l = raw.trimEnd();
      let m;
      if ((m = l.match(/^\s*[-*•]\s+(.*)/))) { if (list !== 'ul') { close(); out += '<ul>'; list = 'ul'; } out += `<li>${inline(m[1])}</li>`; }
      else if ((m = l.match(/^\s*\d+[.)]\s+(.*)/))) { if (list !== 'ol') { close(); out += '<ol>'; list = 'ol'; } out += `<li>${inline(m[1])}</li>`; }
      else if ((m = l.match(/^#{1,6}\s+(.*)/))) { close(); out += `<h4>${inline(m[1])}</h4>`; }
      else if (/^\s*\|/.test(l)) { close(); out += `<div class="tbl">${inline(l)}</div>`; }
      else if (!l.trim()) { close(); }
      else { close(); out += `<p>${inline(l)}</p>`; }
    }
    close();
    return out;
  }
  function clickAnswer(e) {
    const a = e.target.closest?.('a.path');
    if (a) { e.preventDefault(); invoke('reveal', { path: a.dataset.path }).catch(() => invoke('open_path', { path: a.dataset.path })); }
  }

  onMount(() => {
    refreshStatus();
    let un;
    listen('ask-step', (e) => { live = [...live, e.payload]; scrollDown(); }).then((f) => (un = f));
    scrollDown();
    return () => un?.();
  });
</script>

<div class="av">
  {#if settings && !settings.ask_ok}
    <div class="intro card">
      <h1>Ask Claude</h1>
      <p>Ask questions about your PC in plain English: <i>“what can I delete to free 50 GB?”</i>, <i>“where's my AirSim project?”</i>, <i>“is everything backed up?”</i></p>
      <h3>What Claude sees</h3>
      <ul>
        <li>Only what Cartographer already knows: <b>folder, file, app and game names, paths, sizes and dates</b>, plus your projects' git status.</li>
        <li><b>Never the contents of your files.</b> Claude looks things up with a few read-only tools, and every answer lists what it looked at.</li>
        <li>Nothing is sent until you ask a question. Claude <b>can't change anything</b>: archiving or deleting stays a button you press in Cleanup.</li>
      </ul>
      <h3>How it connects</h3>
      <p>{status?.claude_code ? 'Claude Code is installed, so Cartographer uses your Claude login (no API key, nothing extra to pay).' : 'Claude Code isn\'t installed, so you\'ll need an Anthropic API key (billed per question on your API account).'}</p>
      <button class="primary" onclick={onenable}>Turn on Ask Claude</button>
    </div>
  {:else}
    <div class="top">
      <div>
        <h1>Ask Claude</h1>
        <p class="using">Using {usingNow} · <button class="link" onclick={() => (showSetup = !showSetup)}>{showSetup ? 'hide settings' : 'settings'}</button></p>
      </div>
      {#if msgs.length}<button class="ghost" onclick={reset} disabled={busy}>New conversation</button>{/if}
    </div>

    {#if showSetup || (status && !ready)}
      <div class="setup card">
        <label>Connect with
          <select value={settings.ask_backend} onchange={(e) => onbackend?.(e.currentTarget.value)}>
            <option value="auto">Automatic (Claude Code if installed, else API key)</option>
            <option value="claude_code">Claude Code (your Claude login)</option>
            <option value="api">Anthropic API key</option>
          </select>
        </label>
        <div class="row">
          <span class="lbl">Claude Code</span>
          <span class={status?.claude_code ? 'ok' : 'mut'}>{status?.claude_code ? '✓ installed' : 'not installed'}</span>
          {#if status?.claude_code}<span class="mut small">If answers fail with a sign-in error, run <code>claude</code> once in a terminal and log in.</span>{/if}
        </div>
        <div class="row">
          <span class="lbl">API key</span>
          {#if status?.api_key}
            <span class="ok">✓ {status.api_key}</span>
            <button class="ghost" onclick={() => saveKey(true)}>Remove</button>
          {:else}
            <input type="password" placeholder="sk-ant-…" bind:value={keyInput} autocomplete="off" spellcheck="false" />
            <button class="ghost" disabled={!keyInput.trim()} onclick={() => saveKey()}>Save</button>
          {/if}
          <span class="mut small">Stored encrypted for your Windows account. Uses {status?.model ?? 'Claude'}, roughly a few cents per question.</span>
        </div>
      </div>
    {/if}

    <div class="chat" bind:this={scroller}>
      {#if !msgs.length}
        <div class="empty">
          <p>Try one of these:</p>
          <div class="sugs">
            {#each SUGGEST as s (s)}<button onclick={() => send(s)} disabled={busy || !ready}>{s}</button>{/each}
          </div>
        </div>
      {/if}
      {#each msgs as m, i (i)}
        {#if m.role === 'user'}
          <div class="q" class:pvt={privacy.on}>{pv(m.text)}</div>
        {:else if m.role === 'error'}
          <div class="a err">{m.text}</div>
        {:else}
          <div class="a">
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
            <div class="md" class:pvt={privacy.on} onclick={clickAnswer}>{@html privacy.on ? md(pv(m.text)) : md(m.text)}</div>
            <details class="meta">
              <summary>
                via {m.via} · {m.seconds.toFixed(0)} s · looked at {m.steps.length} thing{m.steps.length === 1 ? '' : 's'}{m.cost_usd != null ? ` · ≈ $${m.cost_usd.toFixed(2)}` : ''}
              </summary>
              <ul>
                {#each m.steps as s, j (j)}<li class:pvt={privacy.on}>{privacy.on ? pv(stepText(s)) : stepText(s)}</li>{/each}
                {#if !m.steps.length}<li>Answered without looking anything up.</li>{/if}
              </ul>
            </details>
          </div>
        {/if}
      {/each}
      {#if busy}
        <div class="a thinking">
          <div class="spin"></div>
          <div>
            <b>Claude is looking…</b>
            {#each live as s, j (j)}<span class:pvt={privacy.on}>{privacy.on ? pv(stepText(s)) : stepText(s)}</span>{/each}
          </div>
        </div>
      {/if}
    </div>

    {#if error}<div class="errbar">{error}</div>{/if}
    <form class="composer" onsubmit={(e) => { e.preventDefault(); send(); }}>
      <textarea
        rows="2"
        placeholder={ready ? 'Ask about your drives, projects, games, backups…' : 'Connect Claude in settings first'}
        bind:value={q}
        disabled={!ready}
        onkeydown={(e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send(); } }}
      ></textarea>
      <button class="primary" type="submit" disabled={busy || !ready || !q.trim()}>{busy ? '…' : 'Ask'}</button>
    </form>
    <p class="foot">Claude sees names, paths, sizes and dates from your index, never file contents, and can't change anything.</p>
  {/if}
</div>

<style>
  .av { grid-column: 1 / -1; display: flex; flex-direction: column; min-height: 0; padding: 22px 28px 14px; gap: 12px; }
  h1 { margin: 0 0 4px; font-size: 22px; }
  .card { border: 1px solid var(--line); border-radius: 12px; background: var(--panel2); padding: 16px 18px; }
  .intro { max-width: 720px; overflow-y: auto; }
  .intro p, .intro li { font-size: 13.5px; color: #c4cfde; line-height: 1.55; }
  .intro h3 { margin: 16px 0 4px; font-size: 12px; text-transform: uppercase; letter-spacing: 1px; color: var(--muted); }
  .intro ul { margin: 0; padding-left: 18px; }
  .primary { padding: 8px 16px; border-radius: 9px; background: #12302f; border: 1px solid var(--accent); color: var(--accent); font-weight: 600; font-size: 13px; }
  .primary:disabled { opacity: 0.45; }
  .intro .primary { margin-top: 14px; }
  .ghost { padding: 5px 11px; border-radius: 8px; border: 1px solid var(--line); color: var(--accent); font-size: 12.5px; }
  .ghost:disabled { opacity: 0.4; }
  .top { display: flex; justify-content: space-between; align-items: flex-end; }
  .using { margin: 0; font-size: 12.5px; color: var(--muted); }
  .link { color: var(--accent); font-size: 12.5px; }
  .setup { display: flex; flex-direction: column; gap: 10px; font-size: 13px; }
  .setup select, .setup input { background: var(--bg); border: 1px solid var(--line); border-radius: 8px; color: var(--text); padding: 5px 9px; font-size: 12.5px; margin-left: 8px; }
  .setup input { width: 260px; margin-left: 0; }
  .row { display: flex; gap: 10px; align-items: center; flex-wrap: wrap; }
  .lbl { width: 86px; color: var(--muted); font-size: 12px; }
  .ok { color: #5eead4; }
  .mut { color: var(--muted); }
  .small { font-size: 11.5px; }
  code { background: #172131; padding: 1px 5px; border-radius: 4px; font-size: 12px; }
  .chat { flex: 1; min-height: 0; overflow-y: auto; display: flex; flex-direction: column; gap: 12px; padding: 4px 2px; }
  .empty p { color: var(--muted); font-size: 13px; margin: 6px 0 10px; }
  .sugs { display: flex; flex-wrap: wrap; gap: 8px; }
  .sugs button { padding: 7px 12px; border-radius: 99px; border: 1px solid var(--line); background: var(--panel2); color: #c4cfde; font-size: 12.5px; text-align: left; }
  .sugs button:hover:not(:disabled) { border-color: var(--accent); color: var(--accent); }
  .q { align-self: flex-end; max-width: 75%; background: #12302f; border: 1px solid #1d4a47; border-radius: 12px 12px 3px 12px; padding: 9px 13px; font-size: 13.5px; white-space: pre-wrap; }
  .a { align-self: flex-start; max-width: 88%; background: var(--panel2); border: 1px solid var(--line); border-radius: 12px 12px 12px 3px; padding: 10px 14px; font-size: 13.5px; line-height: 1.55; }
  .a.err { border-color: #6b2229; background: #2a1215; color: #fca5a5; }
  .md :global(p) { margin: 0 0 8px; }
  .md :global(p:last-child) { margin-bottom: 0; }
  .md :global(ul), .md :global(ol) { margin: 2px 0 8px; padding-left: 20px; }
  .md :global(li) { margin: 2px 0; }
  .md :global(h4) { margin: 10px 0 4px; font-size: 13.5px; }
  .md :global(.tbl) { font-family: ui-monospace, Consolas, monospace; font-size: 12px; white-space: pre; overflow-x: auto; color: #c4cfde; }
  .md :global(a.path) { color: var(--accent); cursor: pointer; text-decoration: underline dotted; word-break: break-all; }
  .meta { margin-top: 8px; font-size: 11.5px; color: var(--muted); }
  .meta summary { cursor: pointer; }
  .meta ul { margin: 4px 0 0; padding-left: 18px; }
  .thinking { display: flex; gap: 10px; align-items: flex-start; }
  .thinking div { display: flex; flex-direction: column; gap: 2px; font-size: 12.5px; }
  .thinking span { color: var(--muted); font-size: 12px; }
  .spin { width: 15px; height: 15px; margin-top: 2px; border-radius: 50%; border: 2px solid #1d4a47; border-top-color: var(--accent); animation: spin 0.8s linear infinite; flex: none; }
  .errbar { padding: 8px 12px; border-radius: 8px; background: #2a1215; border: 1px solid #6b2229; color: #fca5a5; font-size: 12.5px; }
  .composer { display: flex; gap: 10px; align-items: stretch; }
  .composer textarea { flex: 1; resize: none; background: var(--panel2); border: 1px solid var(--line); border-radius: 10px; color: var(--text); padding: 9px 12px; font: inherit; font-size: 13.5px; }
  .composer textarea:focus { outline: none; border-color: #2c3b52; }
  .foot { margin: 0; font-size: 11.5px; color: var(--muted); }
</style>
