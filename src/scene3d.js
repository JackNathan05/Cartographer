// 3D view of the map (three.js). Uses the 2D layout's x/y for the ground plane and lifts
// each folder level to its own height, so every drive becomes a glowing "tree".
//
// Two camera modes:
//  - "interactive": orbit / zoom / pan, hover for names, click to fly to a dot
//  - "show": slow cinematic orbit with fly-bys to the biggest or most recently changed
//    dots and a caption (the idle "B-roll")

import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';

const KIND_COLORS = {
  drive: 0xe2e8f0, folder: 0x5b6b84, project: 0x38bdf8, game: 0x4ade80, app: 0xa78bfa,
  service: 0xfb923c, archived: 0x475569, backup: 0x2dd4bf,
};

function glowTexture() {
  const c = document.createElement('canvas');
  c.width = c.height = 128;
  const g = c.getContext('2d');
  const grd = g.createRadialGradient(64, 64, 0, 64, 64, 64);
  grd.addColorStop(0, 'rgba(255,255,255,1)');
  grd.addColorStop(0.25, 'rgba(255,255,255,0.45)');
  grd.addColorStop(1, 'rgba(255,255,255,0)');
  g.fillStyle = grd;
  g.fillRect(0, 0, 128, 128);
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  return t;
}

function textSprite(text) {
  const c = document.createElement('canvas');
  const g = c.getContext('2d');
  const font = '600 44px Segoe UI, sans-serif';
  g.font = font;
  const w = Math.ceil(g.measureText(text).width) + 40;
  c.width = w;
  c.height = 72;
  g.font = font;
  g.fillStyle = 'rgba(9,13,21,0.78)';
  g.beginPath();
  g.roundRect(0, 0, w, 72, 18);
  g.fill();
  g.fillStyle = '#e6edf7';
  g.textBaseline = 'middle';
  g.fillText(text, 20, 38);
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  const s = new THREE.Sprite(new THREE.SpriteMaterial({ map: t, depthWrite: false, transparent: true }));
  s.scale.set(w / 4.5, 16, 1);
  return s;
}

/**
 * @param container element to draw into
 * @param nodes [{id, kind, label, size(bytes), r (drawn radius), x, y, depth, files?}]
 * @param links [{source, target}]
 * @param opts { mode: 'interactive'|'show', fps, mask: (s)=>s, onHover(n|null, x, y), onPick(n), caption(text) }
 */
export function createScene3D(container, nodes, links, opts = {}) {
  const mask = opts.mask ?? ((s) => s);
  const W = () => container.clientWidth || 800;
  const H = () => container.clientHeight || 600;

  const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false, powerPreference: 'low-power' });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 1.5));
  renderer.setSize(W(), H());
  renderer.setClearColor(0x070a12, 1);
  container.appendChild(renderer.domElement);

  const scene = new THREE.Scene();
  scene.fog = new THREE.FogExp2(0x070a12, 0.00042);
  const camera = new THREE.PerspectiveCamera(50, W() / H(), 1, 6000);

  // centre and scale the layout
  let cx = 0, cy = 0;
  for (const n of nodes) { cx += n.x; cy += n.y; }
  cx /= nodes.length || 1; cy /= nodes.length || 1;
  let ext = 1;
  for (const n of nodes) ext = Math.max(ext, Math.abs(n.x - cx), Math.abs(n.y - cy));
  const S = 420 / ext;
  const STEP = 34;
  const pos = new Map();
  const hashf = (s) => { let h = 2166136261; for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619); return ((h >>> 0) % 1000) / 1000; };
  for (const n of nodes) {
    pos.set(n.id, new THREE.Vector3((n.x - cx) * S, n.depth * STEP + (hashf(n.id) - 0.5) * 10, (n.y - cy) * S));
  }

  // lights
  scene.add(new THREE.AmbientLight(0xffffff, 1.5));
  scene.add(new THREE.HemisphereLight(0xbfe9ff, 0x1a1030, 0.9));
  const key = new THREE.PointLight(0x9fe7ff, 2.2, 0, 0);
  key.position.set(0, 600, 300);
  scene.add(key);
  const rim = new THREE.DirectionalLight(0x7c5cff, 0.5);
  rim.position.set(-400, 200, -500);
  scene.add(rim);

  // stars
  {
    const g = new THREE.BufferGeometry();
    const N = 1600, arr = new Float32Array(N * 3);
    for (let i = 0; i < N; i++) {
      const r = 1400 + Math.random() * 1600, t = Math.random() * Math.PI * 2, p = Math.acos(2 * Math.random() - 1);
      arr[i * 3] = r * Math.sin(p) * Math.cos(t); arr[i * 3 + 1] = r * Math.cos(p); arr[i * 3 + 2] = r * Math.sin(p) * Math.sin(t);
    }
    g.setAttribute('position', new THREE.BufferAttribute(arr, 3));
    scene.add(new THREE.Points(g, new THREE.PointsMaterial({ color: 0x8aa0c8, size: 2, sizeAttenuation: false, transparent: true, opacity: 0.55, fog: false })));
  }

  // nodes: instanced spheres + additive glow halos
  const geo = new THREE.SphereGeometry(1, 20, 14);
  const mat = new THREE.MeshStandardMaterial({ roughness: 0.45, metalness: 0.05 });
  const mesh = new THREE.InstancedMesh(geo, mat, nodes.length);
  const color = new THREE.Color();
  const m4 = new THREE.Matrix4();
  const scaleOf = (n) => Math.max(1.6, (n.r ?? 4) * 1.15);
  nodes.forEach((n, i) => {
    const p = pos.get(n.id);
    const s = scaleOf(n);
    m4.makeScale(s, s, s).setPosition(p);
    mesh.setMatrixAt(i, m4);
    color.setHex(KIND_COLORS[n.kind] ?? 0x94a3b8);
    mesh.setColorAt(i, color);
  });
  mesh.instanceColor.needsUpdate = true;
  scene.add(mesh);

  const glowTex = glowTexture();
  const glowGroup = new THREE.Group();
  nodes.forEach((n) => {
    if (n.kind === 'folder') return;
    const sp = new THREE.Sprite(new THREE.SpriteMaterial({
      map: glowTex, color: KIND_COLORS[n.kind] ?? 0x94a3b8, blending: THREE.AdditiveBlending,
      depthWrite: false, transparent: true, opacity: n.kind === 'drive' ? 0.5 : 0.55,
    }));
    const s = scaleOf(n) * (n.kind === 'drive' ? 7 : 5);
    sp.scale.set(s, s, 1);
    sp.position.copy(pos.get(n.id));
    sp.userData.base = s;
    glowGroup.add(sp);
  });
  scene.add(glowGroup);

  // links
  {
    const arr = [], cols = [];
    const ca = new THREE.Color(), cb = new THREE.Color();
    const byId = new Map(nodes.map((n) => [n.id, n]));
    for (const l of links) {
      const a = pos.get(l.source), b = pos.get(l.target);
      if (!a || !b) continue;
      arr.push(a.x, a.y, a.z, b.x, b.y, b.z);
      ca.setHex(KIND_COLORS[byId.get(l.source)?.kind] ?? 0x5b6b84);
      cb.setHex(KIND_COLORS[byId.get(l.target)?.kind] ?? 0x5b6b84);
      cols.push(ca.r, ca.g, ca.b, cb.r, cb.g, cb.b);
    }
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.Float32BufferAttribute(arr, 3));
    g.setAttribute('color', new THREE.Float32BufferAttribute(cols, 3));
    scene.add(new THREE.LineSegments(g, new THREE.LineBasicMaterial({ vertexColors: true, transparent: true, opacity: 0.45, depthWrite: false })));
  }

  // drive labels
  for (const n of nodes) {
    if (n.kind !== 'drive') continue;
    const sp = textSprite(n.label);
    sp.position.copy(pos.get(n.id)).add(new THREE.Vector3(0, scaleOf(n) + 14, 0));
    scene.add(sp);
  }

  // ground grid glow
  {
    const grid = new THREE.PolarGridHelper(520, 16, 8, 64, 0x1b2a44, 0x111a2c);
    grid.position.y = -12;
    grid.material.transparent = true;
    grid.material.opacity = 0.5;
    scene.add(grid);
  }

  // camera + controls
  camera.position.set(0, 330, 470);
  const controls = new OrbitControls(camera, renderer.domElement);
  controls.enableDamping = true;
  controls.dampingFactor = 0.08;
  controls.target.set(0, 60, 0);
  controls.maxDistance = 2200;
  controls.minDistance = 30;

  // hover / pick
  const ray = new THREE.Raycaster();
  const mouse = new THREE.Vector2();
  let hovered = -1;
  function pickAt(ev) {
    const r = renderer.domElement.getBoundingClientRect();
    mouse.x = ((ev.clientX - r.left) / r.width) * 2 - 1;
    mouse.y = -((ev.clientY - r.top) / r.height) * 2 + 1;
    ray.setFromCamera(mouse, camera);
    const hit = ray.intersectObject(mesh, false)[0];
    return hit ? hit.instanceId : -1;
  }
  const onMove = (ev) => {
    if (mode !== 'interactive') return;
    const i = pickAt(ev);
    if (i !== hovered) {
      hovered = i;
      renderer.domElement.style.cursor = i >= 0 ? 'pointer' : '';
    }
    opts.onHover?.(i >= 0 ? nodes[i] : null, ev.clientX, ev.clientY);
  };
  const onClick = (ev) => {
    if (mode !== 'interactive') return;
    const i = pickAt(ev);
    if (i >= 0) { flyTo(nodes[i], 2.2); opts.onPick?.(nodes[i]); }
  };
  renderer.domElement.addEventListener('pointermove', onMove);
  renderer.domElement.addEventListener('click', onClick);

  // camera animation
  let anim = null; // { from: {p,t}, to: {p,t}, t0, dur }
  function flyTo(n, closeness = 1) {
    const p = pos.get(n.id);
    const dist = Math.max(60, scaleOf(n) * 14) / closeness * 2.2;
    const dir = new THREE.Vector3().subVectors(camera.position, controls.target).normalize();
    const to = { t: p.clone(), p: p.clone().addScaledVector(dir, dist).add(new THREE.Vector3(0, dist * 0.35, 0)) };
    anim = { from: { p: camera.position.clone(), t: controls.target.clone() }, to, t0: performance.now(), dur: 1600 };
  }
  const ease = (x) => (x < 0.5 ? 4 * x * x * x : 1 - Math.pow(-2 * x + 2, 3) / 2);

  // show mode choreography
  let mode = opts.mode ?? 'interactive';
  let orbitA = 0;
  let nextBeat = 0;
  let beatIdx = 0;
  const stars = [...nodes].filter((n) => n.kind !== 'folder' && n.kind !== 'drive').sort((a, b) => b.size - a.size);
  const recent = [...nodes].filter((n) => n.newest).sort((a, b) => (b.newest ?? 0) - (a.newest ?? 0));
  const fmtGB = (b) => (b >= 1073741824 ? `${(b / 1073741824).toFixed(1)} GB` : `${Math.round(b / 1048576)} MB`);
  function showBeat(now) {
    // alternate: wide orbit (12 s) -> fly-by to a highlight (10 s)
    beatIdx++;
    if (beatIdx % 2 === 1 && stars.length) {
      const pool = beatIdx % 4 === 1 ? stars.slice(0, 12) : recent.slice(0, 12);
      const n = pool[Math.floor(Math.random() * pool.length)] ?? stars[0];
      flyTo(n, 1.1);
      anim.dur = 4200;
      opts.caption?.(`${mask(n.label)} · ${fmtGB(n.size)}${n.files ? ` · ${n.files.toLocaleString('en-US')} files` : ''}`);
      nextBeat = now + 11000;
    } else {
      const r = 480 + Math.random() * 180;
      orbitA = Math.atan2(camera.position.z, camera.position.x);
      anim = { from: { p: camera.position.clone(), t: controls.target.clone() }, to: { t: new THREE.Vector3(0, 60, 0), p: new THREE.Vector3(Math.cos(orbitA) * r, 300 + Math.random() * 160, Math.sin(orbitA) * r) }, t0: now, dur: 4500 };
      opts.caption?.('');
      nextBeat = now + 14000;
    }
  }

  // render loop (frame-capped)
  let raf = 0, last = 0, alive = true;
  const fps = () => (mode === 'show' ? 30 : opts.fps ?? 60);
  function frame(now) {
    if (!alive) return;
    raf = requestAnimationFrame(frame);
    if (now - last < 1000 / fps() - 1) return;
    const dt = Math.min(0.1, (now - last) / 1000);
    last = now;

    if (mode === 'show') {
      if (now > nextBeat) showBeat(now);
      if (!anim) {
        // slow drift around the target between beats
        orbitA += dt * 0.06;
        const off = new THREE.Vector3().subVectors(camera.position, controls.target);
        const rad = Math.hypot(off.x, off.z);
        camera.position.x = controls.target.x + Math.cos(orbitA) * rad;
        camera.position.z = controls.target.z + Math.sin(orbitA) * rad;
        camera.position.y += Math.sin(now / 4000) * 0.08;
        camera.lookAt(controls.target);
      }
      // gentle pulse on the halos
      const k = 1 + Math.sin(now / 900) * 0.08;
      glowGroup.children.forEach((sp, i) => { const b = sp.userData.base * (1 + Math.sin(now / 1100 + i) * 0.06) * k; sp.scale.set(b, b, 1); });
    }
    if (anim) {
      const t = Math.min(1, (now - anim.t0) / anim.dur), e = ease(t);
      camera.position.lerpVectors(anim.from.p, anim.to.p, e);
      controls.target.lerpVectors(anim.from.t, anim.to.t, e);
      camera.lookAt(controls.target);
      if (t >= 1) { anim = null; orbitA = Math.atan2(camera.position.z - controls.target.z, camera.position.x - controls.target.x); }
    }
    if (mode === 'interactive') controls.update();
    renderer.render(scene, camera);
  }
  raf = requestAnimationFrame(frame);

  const ro = new ResizeObserver(() => {
    renderer.setSize(W(), H());
    camera.aspect = W() / H();
    camera.updateProjectionMatrix();
  });
  ro.observe(container);

  function setMode(m) {
    mode = m;
    controls.enabled = m === 'interactive';
    if (m === 'show') { nextBeat = 0; beatIdx = 0; }
  }
  setMode(mode);

  return {
    setMode,
    flyToId(id) { const n = nodes.find((x) => x.id === id); if (n) flyTo(n, 1.6); },
    reset() { anim = { from: { p: camera.position.clone(), t: controls.target.clone() }, to: { p: new THREE.Vector3(0, 330, 470), t: new THREE.Vector3(0, 60, 0) }, t0: performance.now(), dur: 1200 }; },
    dispose() {
      alive = false;
      cancelAnimationFrame(raf);
      ro.disconnect();
      renderer.domElement.removeEventListener('pointermove', onMove);
      renderer.domElement.removeEventListener('click', onClick);
      controls.dispose();
      scene.traverse((o) => { o.geometry?.dispose?.(); if (o.material) { o.material.map?.dispose?.(); o.material.dispose?.(); } });
      renderer.dispose();
      renderer.domElement.remove();
    },
  };
}
