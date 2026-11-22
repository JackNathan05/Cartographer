export function fmtBytes(b) {
  if (b == null || isNaN(b)) return '';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  let v = b;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${v >= 100 || i === 0 ? Math.round(v) : v.toFixed(1)} ${u[i]}`;
}

export function fmtCount(n) {
  return (n ?? 0).toLocaleString('en-US');
}

export function fmtAgo(secs) {
  if (!secs) return '';
  const d = Date.now() / 1000 - secs;
  if (d < 60) return 'just now';
  if (d < 3600) return `${Math.round(d / 60)} min ago`;
  if (d < 86400) return `${Math.round(d / 3600)} h ago`;
  if (d < 86400 * 45) return `${Math.round(d / 86400)} days ago`;
  if (d < 86400 * 365) return `${Math.round(d / (86400 * 30))} months ago`;
  const y = d / (86400 * 365);
  return `${y < 10 ? y.toFixed(1) : Math.round(y)} years ago`;
}

export function fmtDate(secs) {
  if (!secs) return '';
  return new Date(secs * 1000).toLocaleDateString('en-US', { year: 'numeric', month: 'short', day: 'numeric' });
}

/** Stable, pleasant hue for the i-th top-level block. */
export function hueFor(i) {
  return (195 + i * 137.508) % 360;
}
