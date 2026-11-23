// Live map layout: a d3-force simulation with Cartographer's own forces.
//
// - "lives in" links pull each item toward its folder / drive
// - everything repels a little (so dots don't stack), and nodes from DIFFERENT drives
//   repel each other strongly (so each drive keeps its own territory)
// - "radiate" points push the map outward in rings: drives sit closest to a radiate
//   point, and each folder level sits one ring further out
// - "pull" / "push" points the user places attract or repel anything within their reach
// - nodes the user drags are pinned where they're dropped
//
// Everything here is plain data so it can be saved to layout.json and replayed.

import { forceSimulation, forceLink, forceManyBody, forceCollide, forceX, forceY } from 'd3-force';

export const DEFAULTS = {
  linkDistance: 34, // how far an item sits from its folder
  linkStrength: 0.7, // how hard it's held there
  repulsion: 45, // general push between all dots
  driveRepel: 1.2, // extra push between dots of different drives
  driveRange: 260, // how far that push reaches
  ring: 62, // spacing between rings when radiating outward
  core: 90, // how far from a radiate point the drives sit
  radiate: 0.12, // how strongly rings are kept
  gravity: 0.015, // gentle pull to the middle so nothing drifts away
  straighten: 0, // pull each node's branches to line up across from its root branch (0 = off)
  untangle: 0.6, // keep branches from crossing, and dots off other branches' lines
};

export const SLIDERS = [
  { key: 'untangle', label: 'Untangle branches (no overlaps)', min: 0, max: 1, step: 0.01 },
  { key: 'straighten', label: 'Straighten branches', min: 0, max: 1, step: 0.01 },
  { key: 'ring', label: 'Ring spacing', min: 20, max: 160, step: 1 },
  { key: 'radiate', label: 'Radiate strength', min: 0, max: 0.5, step: 0.01 },
  { key: 'core', label: 'Drive distance from center', min: 0, max: 300, step: 1 },
  { key: 'driveRepel', label: 'Push between drives', min: 0, max: 4, step: 0.05 },
  { key: 'driveRange', label: 'Drive push reach', min: 50, max: 800, step: 10 },
  { key: 'repulsion', label: 'Push between dots', min: 0, max: 200, step: 1 },
  { key: 'linkDistance', label: 'Link length', min: 5, max: 150, step: 1 },
  { key: 'linkStrength', label: 'Link strength', min: 0, max: 1, step: 0.01 },
  { key: 'gravity', label: 'Pull to middle', min: 0, max: 0.2, step: 0.005 },
];

export const POINT_TYPES = {
  radiate: { label: 'Radiate from', color: '#fbbf24' },
  pull: { label: 'Pull toward', color: '#22d3ee' },
  push: { label: 'Push away', color: '#f472b6' },
};

export function defaultLayout() {
  return {
    version: 1,
    settings: { ...DEFAULTS },
    points: [{ id: 'center', type: 'radiate', x: 0, y: 0, strength: 1, reach: 0 }],
    pinned: {}, // node id -> [x, y]
    aim: {}, // node id -> point id its branches straighten away from (instead of its parent)
    positions: {}, // node id -> [x, y] (last saved, used as the starting point)
  };
}

function hash(s) {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619);
  return ((h >>> 0) % 100000) / 100000;
}

/**
 * @param data   graph data from Rust ({nodes, links})
 * @param layout saved layout (see defaultLayout)
 * @param onTick called after every simulation step
 */
export function createLayout(data, layout, onTick) {
  const parentOf = new Map();
  for (const l of data.links) if (l.kind === 'lives_on') parentOf.set(l.source, l.target);
  const driveOf = (id) => {
    let cur = id;
    for (let i = 0; i < 64 && parentOf.has(cur); i++) cur = parentOf.get(cur);
    return cur.startsWith('drive:') ? cur : id;
  };
  const depthOf = (id) => {
    let d = 0, cur = id;
    while (parentOf.has(cur) && d < 64) { cur = parentOf.get(cur); d++; }
    return d;
  };

  const drives = data.nodes.filter((n) => n.kind === 'drive').map((n) => n.id);
  const nodes = data.nodes.map((n) => ({
    id: n.id,
    kind: n.kind,
    size: n.size,
    drive: driveOf(n.id),
    depth: depthOf(n.id),
  }));
  const byId = new Map(nodes.map((n) => [n.id, n]));

  // starting positions: last saved spot, else around a radiate point in the drive's direction
  const di = new Map(drives.map((d, i) => [d, i]));
  for (const n of nodes) {
    const saved = layout.pinned[n.id] ?? layout.positions[n.id];
    if (saved) { n.x = saved[0]; n.y = saved[1]; }
    else {
      const a = ((di.get(n.drive) ?? 0) / Math.max(1, drives.length)) * Math.PI * 2 + (hash(n.id) - 0.5) * 1.2;
      const r = layout.settings.core + n.depth * layout.settings.ring + hash(n.id + '#') * 20;
      n.x = Math.cos(a) * r;
      n.y = Math.sin(a) * r;
    }
    const pin = layout.pinned[n.id];
    if (pin) { n.fx = pin[0]; n.fy = pin[1]; }
  }

  const links = data.links
    .filter((l) => l.kind === 'lives_on' && byId.has(l.source) && byId.has(l.target))
    .map((l) => ({ source: l.source, target: l.target }));

  const s = () => layout.settings;

  // nodes of different drives push apart
  function driveRepel(alpha) {
    const k = s().driveRepel * alpha, range = s().driveRange;
    if (!k) return;
    for (let i = 0; i < nodes.length; i++) {
      const a = nodes[i];
      for (let j = i + 1; j < nodes.length; j++) {
        const b = nodes[j];
        if (a.drive === b.drive) continue;
        let dx = b.x - a.x, dy = b.y - a.y;
        let d = Math.hypot(dx, dy) || 0.01;
        if (d > range) continue;
        const f = (k * (1 - d / range) * 6) / d;
        dx *= f; dy *= f;
        b.vx += dx; b.vy += dy;
        a.vx -= dx; a.vy -= dy;
      }
    }
  }

  // radiate points: each node is held on a ring around its nearest radiate point
  function radiate(alpha) {
    const pts = layout.points.filter((p) => p.type === 'radiate');
    if (!pts.length) return;
    const st = s();
    for (const n of nodes) {
      let best = pts[0], bd = Infinity;
      for (const p of pts) {
        const d = (n.x - p.x) ** 2 + (n.y - p.y) ** 2;
        if (d < bd) { bd = d; best = p; }
      }
      const dx = n.x - best.x, dy = n.y - best.y;
      const r = Math.hypot(dx, dy) || 0.01;
      const target = st.core + n.depth * st.ring;
      const k = ((r - target) / r) * st.radiate * (best.strength ?? 1) * alpha;
      n.vx -= dx * k;
      n.vy -= dy * k;
    }
  }

  // straighten branches: for every node, its children are pulled toward the spot directly
  // across from its root branch (the line back to its parent, or for a drive, to the nearest
  // radiate point). The biggest child pulls hardest; each smaller one a bit less.
  const kidsOf = new Map();
  for (const l of links) {
    const t = typeof l.target === 'object' ? l.target.id : l.target;
    const sId = typeof l.source === 'object' ? l.source.id : l.source;
    if (!kidsOf.has(t)) kidsOf.set(t, []);
    kidsOf.get(t).push(sId);
  }
  for (const [k, list] of kidsOf) {
    kidsOf.set(k, list.map((id) => byId.get(id)).filter(Boolean).sort((a, b) => b.size - a.size || a.id.localeCompare(b.id)));
  }
  function straighten(alpha) {
    const k = s().straighten;
    if (!k) return;
    const radiators = layout.points.filter((p) => p.type === 'radiate');
    for (const [pid, kids] of kidsOf) {
      const n = byId.get(pid);
      if (!n || !kids.length) continue;
      // where the root branch comes from: a point the user picked, else the parent,
      // else (drives) the nearest radiate point
      let rx, ry;
      const par = parentOf.get(pid);
      const aimed = layout.aim?.[pid] && layout.points.find((p) => p.id === layout.aim[pid]);
      if (aimed) { rx = aimed.x; ry = aimed.y; }
      else if (par && byId.has(par)) { const pn = byId.get(par); rx = pn.x; ry = pn.y; }
      else if (radiators.length) {
        let best = radiators[0], bd = Infinity;
        for (const p of radiators) { const d = (n.x - p.x) ** 2 + (n.y - p.y) ** 2; if (d < bd) { bd = d; best = p; } }
        rx = best.x; ry = best.y;
      } else continue;
      let ux = n.x - rx, uy = n.y - ry;
      const ul = Math.hypot(ux, uy);
      if (ul < 0.01) continue;
      ux /= ul; uy /= ul;
      kids.forEach((c, i) => {
        if (c.fx != null) return;
        const dist = Math.hypot(c.x - n.x, c.y - n.y) || 1;
        const tx = n.x + ux * dist, ty = n.y + uy * dist;
        const w = Math.min(0.9, (k * k * alpha * 2.5) / (1 + i * 0.6)); // biggest child first, then descending
        c.vx += (tx - c.x) * w;
        c.vy += (ty - c.y) * w;
      });
    }
  }

  // untangle: (1) two branch lines that cross push each other apart, (2) a dot sitting on
  // (or very near) a line from another branch is pushed off it. Pinned dots don't move.
  const segs = () => links.map((l) => [l.source, l.target]).filter(([a, b]) => typeof a === 'object' && typeof b === 'object');
  const cross = (a, b, c, d) => {
    const o = (p, q, r) => (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
    const d1 = o(c, d, a), d2 = o(c, d, b), d3 = o(a, b, c), d4 = o(a, b, d);
    return d1 * d2 < 0 && d3 * d4 < 0;
  };
  const nudge = (n, x, y) => { if (n.fx == null) { n.vx += x; n.vy += y; } };
  function untangle(alpha) {
    const k = s().untangle;
    if (!k) return;
    const S = segs();
    // 1. crossing branches
    for (let i = 0; i < S.length; i++) {
      const [a, b] = S[i];
      for (let j = i + 1; j < S.length; j++) {
        const [c, d] = S[j];
        if (a === c || a === d || b === c || b === d) continue;
        if (!cross(a, b, c, d)) continue;
        let dx = (a.x + b.x - c.x - d.x) / 2, dy = (a.y + b.y - c.y - d.y) / 2;
        let len = Math.hypot(dx, dy);
        if (len < 0.01) { dx = 1; dy = 0; len = 1; }
        const f = (k * alpha * 4) / len;
        // the child ends move most; parents move a little
        nudge(a, dx * f, dy * f); nudge(b, dx * f * 0.3, dy * f * 0.3);
        nudge(c, -dx * f, -dy * f); nudge(d, -dx * f * 0.3, -dy * f * 0.3);
      }
    }
    // 2. dots on other branches' lines
    for (const n of nodes) {
      const r = 10 + Math.min(18, Math.sqrt(Math.max(n.size, 0) / 1073741824) * 2.2);
      for (const [a, b] of S) {
        if (n === a || n === b) continue;
        const ex = b.x - a.x, ey = b.y - a.y;
        const L2 = ex * ex + ey * ey || 1;
        const t = Math.max(0, Math.min(1, ((n.x - a.x) * ex + (n.y - a.y) * ey) / L2));
        if (t <= 0.02 || t >= 0.98) continue;
        const px = a.x + ex * t, py = a.y + ey * t;
        let dx = n.x - px, dy = n.y - py;
        const dist = Math.hypot(dx, dy);
        if (dist >= r) continue;
        if (dist < 0.01) { dx = -ey; dy = ex; } // exactly on the line: push sideways
        const m = Math.hypot(dx, dy) || 1;
        const f = (k * alpha * (r - dist) * 0.8) / m;
        nudge(n, dx * f, dy * f);
        nudge(a, -dx * f * 0.25, -dy * f * 0.25);
        nudge(b, -dx * f * 0.25, -dy * f * 0.25);
      }
    }
  }

  // pull / push points with a reach (0 = everywhere)
  function points(alpha) {
    for (const p of layout.points) {
      if (p.type === 'radiate') continue;
      const sign = p.type === 'pull' ? 1 : -1;
      for (const n of nodes) {
        const dx = p.x - n.x, dy = p.y - n.y;
        const d = Math.hypot(dx, dy) || 0.01;
        if (p.reach > 0 && d > p.reach) continue;
        const fall = p.reach > 0 ? 1 - d / p.reach : 1;
        const k = sign * 0.05 * (p.strength ?? 1) * fall * alpha;
        if (sign > 0) { n.vx += dx * k; n.vy += dy * k; }
        else { n.vx += (dx / d) * k * 120; n.vy += (dy / d) * k * 120; }
      }
    }
  }

  const link = forceLink(links).id((d) => d.id);
  const charge = forceManyBody().distanceMax(350);
  const sim = forceSimulation(nodes)
    .force('link', link)
    .force('charge', charge)
    .force('collide', forceCollide((d) => (d.kind === 'drive' ? 40 : 4 + Math.min(18, Math.sqrt(Math.max(d.size, 0) / 1073741824) * 2.2))).strength(0.9))
    .force('driveRepel', driveRepel)
    .force('radiate', radiate)
    .force('points', points)
    .force('straighten', straighten)
    .force('untangle', untangle)
    .force('gx', forceX(0))
    .force('gy', forceY(0))
    .alphaDecay(0.03)
    .on('tick', () => onTick?.());

  function apply() {
    const st = s();
    link.distance((l) => st.linkDistance * (l.target.kind === 'drive' ? 1.4 : 1)).strength(st.linkStrength);
    charge.strength((d) => -st.repulsion * (d.kind === 'drive' ? 3 : 1));
    sim.force('gx').strength(st.gravity);
    sim.force('gy').strength(st.gravity);
  }
  apply();
  sim.alpha(Object.keys(layout.positions).length ? 0.35 : 1).restart();

  return {
    nodes,
    byId,
    sim,
    /** settings or points changed */
    update(heat = 0.5) {
      apply();
      sim.alpha(Math.max(sim.alpha(), heat)).restart();
    },
    pin(id, x, y) {
      const n = byId.get(id);
      if (!n) return;
      n.fx = x; n.fy = y;
      layout.pinned[id] = [x, y];
    },
    unpin(id) {
      const n = byId.get(id);
      if (n) { n.fx = null; n.fy = null; }
      delete layout.pinned[id];
      sim.alpha(Math.max(sim.alpha(), 0.3)).restart();
    },
    unpinAll() {
      for (const id of Object.keys(layout.pinned)) { const n = byId.get(id); if (n) { n.fx = null; n.fy = null; } }
      layout.pinned = {};
      sim.alpha(0.5).restart();
    },
    snapshot() {
      const positions = {};
      for (const n of nodes) positions[n.id] = [Math.round(n.x * 10) / 10, Math.round(n.y * 10) / 10];
      return { ...layout, positions };
    },
    stop() { sim.stop(); },
  };
}
