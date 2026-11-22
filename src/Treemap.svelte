<script>
  import { hierarchy, treemap, treemapSquarify } from 'd3-hierarchy';
  import { fmtBytes, hueFor } from './format.js';

  let { node, onopen, onhover } = $props();
  let w = $state(0);
  let h = $state(0);

  const layout = $derived.by(() => {
    if (!node || w < 20 || h < 20) return { cells: [], hues: new Map() };
    const root = hierarchy(node, (d) => d.children)
      .sum((d) => (d.children && d.children.length ? 0 : Math.max(d.size, 0)))
      .sort((a, b) => b.value - a.value);
    treemap()
      .tile(treemapSquarify.ratio(1.15))
      .size([w, h])
      .paddingOuter(3)
      .paddingTop((d) => (d.depth === 1 && d.x1 - d.x0 > 50 && d.y1 - d.y0 > 40 ? 20 : 3))
      .paddingInner(2)
      .round(true)(root);
    const hues = new Map();
    (root.children ?? []).forEach((c, i) => hues.set(c, hueFor(i)));
    const cells = root.descendants().filter((d) => d.depth > 0 && d.x1 - d.x0 >= 1 && d.y1 - d.y0 >= 1);
    return { cells, hues };
  });

  function top(d) {
    let n = d;
    while (n.depth > 1) n = n.parent;
    return n;
  }

  function fill(d) {
    const k = d.data.kind;
    if (k === 'hidden') return 'url(#hatch)';
    if (k === 'other') return 'hsl(220 14% 22%)';
    const hue = layout.hues.get(top(d)) ?? 200;
    if (d.depth === 1) return d.children ? `hsl(${hue} 38% 17%)` : `hsl(${hue} 48% ${k === 'file' ? 34 : 30}%)`;
    return `hsl(${hue} ${k === 'file' ? 42 : 52}% ${k === 'file' ? 40 : 34}%)`;
  }

  function stroke(d) {
    if (d.data.kind === 'hidden' || d.data.kind === 'other') return 'hsl(220 14% 30%)';
    const hue = layout.hues.get(top(d)) ?? 200;
    return `hsl(${hue} 60% ${d.depth === 1 ? 42 : 52}%)`;
  }

  function fit(text, px) {
    const max = Math.floor((px - 10) / 7);
    if (max < 3) return '';
    return text.length > max ? text.slice(0, max - 1) + '…' : text;
  }

  function click(d) {
    if (d.data.kind === 'dir' && d.data.id != null) onopen?.(d.data.id);
  }
</script>

<div class="tm" bind:clientWidth={w} bind:clientHeight={h} onmouseleave={() => onhover?.(null)} role="presentation">
  <svg width={w} height={h}>
    <defs>
      <pattern id="hatch" width="8" height="8" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
        <rect width="8" height="8" fill="hsl(220 12% 16%)" />
        <line x1="0" y1="0" x2="0" y2="8" stroke="hsl(220 10% 26%)" stroke-width="3" />
      </pattern>
    </defs>
    {#each layout.cells as d (d.data.kind + (d.data.id ?? d.data.path ?? d.data.name) + d.depth)}
      {@const cw = d.x1 - d.x0}
      {@const ch = d.y1 - d.y0}
      <g
        class="cell"
        class:dir={d.data.kind === 'dir'}
        transform="translate({d.x0},{d.y0})"
        onclick={() => click(d)}
        onmouseenter={() => onhover?.(d.data)}
        role="button"
        tabindex="-1"
        onkeydown={() => {}}
      >
        <rect width={cw} height={ch} rx="4" fill={fill(d)} stroke={stroke(d)} stroke-opacity={d.depth === 1 ? 0.55 : 0.35} />
        {#if d.depth === 1 && d.children && cw > 50 && ch > 40}
          <text x="7" y="14" class="hdr">
            {fit(d.data.name, cw - 60)}<tspan class="sz" dx="6">{cw > 120 ? fmtBytes(d.data.size) : ''}</tspan>
          </text>
        {:else if !d.children && cw > 46 && ch > 22}
          <text x="6" y="15" class="lbl">{fit(d.data.name, cw)}</text>
          {#if ch > 38 && cw > 46}
            <text x="6" y="30" class="lbl sz">{fmtBytes(d.data.size)}</text>
          {/if}
        {/if}
      </g>
    {/each}
  </svg>
</div>

<style>
  .tm { position: absolute; inset: 0; overflow: hidden; }
  svg { display: block; animation: fade 0.25s ease-out; }
  @keyframes fade { from { opacity: 0; transform: scale(0.995); } to { opacity: 1; transform: none; } }
  .cell rect { transition: filter 0.12s ease; }
  .cell:hover rect { filter: brightness(1.35); }
  .cell.dir { cursor: zoom-in; }
  text { pointer-events: none; font-size: 12px; fill: #e6edf7; }
  .hdr { font-weight: 600; font-size: 12px; }
  .sz { fill: #9fb0c8; font-weight: 400; }
</style>
