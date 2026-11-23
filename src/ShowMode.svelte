<script>
  // The fullscreen idle "B-roll": the 3D map with a slow cinematic camera.
  // Any key, click, scroll or real mouse movement closes it.
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import { createLayout, defaultLayout, DEFAULTS } from './layout.js';
  import { mask } from './privacy.svelte.js';

  let box;
  let caption = $state('');
  let shownCaption = $state('');
  let clock = $state('');
  let ready = $state(false);

  function nodeSize(n) {
    if (n.kind === 'drive') return 18;
    const gb = Math.max(n.size, 0) / 1073741824;
    const s = Math.min(22, 3 + Math.sqrt(gb) * 2.2);
    return n.kind === 'folder' ? Math.max(3, s * 0.6) : s;
  }

  function exit() {
    invoke('show_exit');
  }

  onMount(() => {
    let scene = null;
    let t0 = performance.now();
    let start = null;
    const onMove = (e) => {
      if (performance.now() - t0 < 1500) return; // ignore the jitter when the window opens
      if (!start) { start = [e.screenX, e.screenY]; return; }
      if (Math.hypot(e.screenX - start[0], e.screenY - start[1]) > 8) exit();
    };
    window.addEventListener('keydown', exit, true);
    window.addEventListener('mousedown', exit, true);
    window.addEventListener('wheel', exit, true);
    window.addEventListener('mousemove', onMove, true);
    const tick = setInterval(() => (clock = new Date().toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })), 1000);
    clock = new Date().toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });

    (async () => {
      const [data, saved, settings, status] = await Promise.all([
        invoke('graph'), invoke('load_layout'), invoke('get_settings'), invoke('show_status'),
      ]);
      const hide = settings.privacy || (settings.privacy_with_obs && status.obs);
      const base = defaultLayout();
      const layout = {
        ...base, ...(saved ?? {}), settings: { ...DEFAULTS, ...(saved?.settings ?? {}) },
        points: saved?.points ?? base.points, pinned: saved?.pinned ?? {}, positions: saved?.positions ?? {}, aim: saved?.aim ?? {},
      };
      const byId = new Map(data.nodes.map((n) => [n.id, n]));
      const eng = createLayout(data, layout, null, nodeSize);
      eng.sim.stop();
      eng.sim.tick(Object.keys(layout.positions).length ? 60 : 300);
      const nodes = eng.nodes.map((n) => {
        const d = byId.get(n.id);
        return { id: n.id, kind: n.kind, label: d?.label ?? n.id, size: n.size, r: n.r, x: n.x, y: n.y, depth: n.depth, newest: d?.newest ?? 0 };
      });
      const { createScene3D } = await import('./scene3d.js');
      scene = createScene3D(box, nodes, data.links.filter((l) => l.kind === 'lives_on'), {
        mode: 'show',
        mask: (s) => (hide ? mask(s) : s),
        caption: (c) => {
          caption = c;
          shownCaption = '';
          if (c) setTimeout(() => (shownCaption = c), 1800); // appear once the camera arrives
        },
      });
      ready = true;
    })();

    return () => {
      clearInterval(tick);
      window.removeEventListener('keydown', exit, true);
      window.removeEventListener('mousedown', exit, true);
      window.removeEventListener('wheel', exit, true);
      window.removeEventListener('mousemove', onMove, true);
      scene?.dispose();
    };
  });
</script>

<div class="show" class:ready>
  <div class="stage" bind:this={box}></div>
  <div class="vignette"></div>
  <div class="brand"><img src="/icon.png" alt="" /> Cartographer</div>
  <div class="clock">{clock}</div>
  {#if shownCaption && shownCaption === caption}
    <div class="caption">{shownCaption}</div>
  {/if}
</div>

<style>
  .show { position: fixed; inset: 0; background: #070a12; cursor: none; opacity: 0; transition: opacity 2.5s ease; }
  .show.ready { opacity: 1; }
  .stage { position: absolute; inset: 0; }
  .vignette { position: absolute; inset: 0; pointer-events: none; background: radial-gradient(ellipse at 50% 50%, transparent 55%, rgba(0,0,0,0.65) 100%); }
  .brand { position: absolute; left: 36px; top: 30px; display: flex; align-items: center; gap: 12px; font-size: 18px; font-weight: 600; letter-spacing: 0.4px; color: rgba(230,237,247,0.72); }
  .brand img { width: 30px; height: 30px; border-radius: 8px; opacity: 0.9; }
  .clock { position: absolute; right: 40px; top: 28px; font-size: 30px; font-weight: 300; color: rgba(230,237,247,0.6); }
  .caption { position: absolute; left: 50%; bottom: 72px; transform: translateX(-50%); padding: 12px 22px; border-radius: 12px; background: rgba(9,13,21,0.55); border: 1px solid rgba(94,234,212,0.25); font-size: 20px; color: #e6edf7; letter-spacing: 0.3px; animation: rise 1.2s ease; white-space: nowrap; }
  @keyframes rise { from { opacity: 0; transform: translate(-50%, 12px); } to { opacity: 1; transform: translate(-50%, 0); } }
</style>
