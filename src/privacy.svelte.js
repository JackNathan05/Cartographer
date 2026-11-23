// Privacy mode: the real map, every name hidden.
//
// Names are REPLACED with random letters of the same shape before anything is drawn, and
// then blurred. The real text never reaches the screen, so it can't be recovered from a
// screenshot or a stream. Drive letters ("C:\") stay readable.

export const privacy = $state({ on: false, auto: false, obs: false });

function rng(seed) {
  let h = 2166136261;
  for (let i = 0; i < seed.length; i++) h = Math.imul(h ^ seed.charCodeAt(i), 16777619);
  return () => {
    h ^= h << 13; h ^= h >>> 17; h ^= h << 5;
    return ((h >>> 0) % 10000) / 10000;
  };
}

/** Same input -> same scramble, so the screen doesn't flicker. */
export function mask(s) {
  if (!s) return s;
  const r = rng(s);
  let out = '';
  let start = 0;
  if (/^[A-Za-z]:\\/.test(s)) { out = s.slice(0, 3); start = 3; }
  for (let i = start; i < s.length; i++) {
    const c = s[i];
    if (/[a-z]/.test(c)) out += String.fromCharCode(97 + Math.floor(r() * 26));
    else if (/[A-Z]/.test(c)) out += String.fromCharCode(65 + Math.floor(r() * 26));
    else if (/[0-9]/.test(c)) out += String(Math.floor(r() * 10));
    else if (/[\s\\/:.\-_()·]/.test(c)) out += c;
    else out += String.fromCharCode(97 + Math.floor(r() * 26));
  }
  return out;
}

/** Use for any file / folder / app name or path shown on screen. */
export function pv(s) {
  return privacy.on ? mask(s) : s;
}
