//! Fast scan: read the NTFS Master File Table directly (like WizTree / Everything).
//! Needs admin (raw volume access), so it runs inside the elevated helper.
//!
//! Every file and folder on an NTFS volume has a 1 KB record in $MFT holding its name(s),
//! parent folder, timestamps and size. Reading that one file sequentially is far faster
//! than asking Windows for each folder's listing. Hard links are counted once.
//! Output is the same `scan::Dir` tree the folder walk produces, so the rest of the app
//! doesn't care which scanner ran.

use crate::scan::{Dir, FileEnt, Progress, BIG_FILE};
use rayon::prelude::*;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::Ordering;

const ROOT: usize = 5; // record number of the volume's root folder
const ATTR_STANDARD_INFO: u32 = 0x10;
const ATTR_FILE_NAME: u32 = 0x30;
const ATTR_DATA: u32 = 0x80;
const ATTR_END: u32 = 0xFFFF_FFFF;
const REF_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const CHUNK: usize = 8 * 1024 * 1024;

fn u16le(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) }
fn u32le(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) }
fn u64le(b: &[u8], o: usize) -> u64 { u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) }

fn ft_to_unix(ft: u64) -> i64 {
    if ft == 0 { 0 } else { (ft / 10_000_000) as i64 - 11_644_473_600 }
}

/// Undo the "update sequence" protection NTFS puts on the last 2 bytes of every sector.
fn fixup(rec: &mut [u8], sector: usize) -> bool {
    if rec.len() < 48 || &rec[0..4] != b"FILE" {
        return false;
    }
    let off = u16le(rec, 4) as usize;
    let cnt = u16le(rec, 6) as usize;
    if cnt == 0 || off + cnt * 2 > rec.len() {
        return false;
    }
    let (a, b) = (rec[off], rec[off + 1]);
    for i in 1..cnt {
        let p = i * sector - 2;
        if p + 2 > rec.len() || rec[p] != a || rec[p + 1] != b {
            return false;
        }
        rec[p] = rec[off + 2 * i];
        rec[p + 1] = rec[off + 2 * i + 1];
    }
    true
}

/// Decode a non-resident attribute's run list into (lcn, clusters); lcn < 0 = sparse.
fn data_runs(b: &[u8], mut o: usize) -> Vec<(i64, u64)> {
    let mut runs = Vec::new();
    let mut lcn: i64 = 0;
    while o < b.len() {
        let h = b[o];
        if h == 0 {
            break;
        }
        let ll = (h & 0x0F) as usize;
        let ol = (h >> 4) as usize;
        o += 1;
        if ll == 0 || ll > 8 || ol > 8 || o + ll + ol > b.len() {
            break;
        }
        let mut len: u64 = 0;
        for i in 0..ll {
            len |= (b[o + i] as u64) << (8 * i);
        }
        o += ll;
        if ol == 0 {
            runs.push((-1, len));
        } else {
            let mut off: i64 = 0;
            for i in 0..ol {
                off |= (b[o + i] as i64) << (8 * i);
            }
            if ol < 8 && b[o + ol - 1] & 0x80 != 0 {
                off |= -1i64 << (8 * ol); // sign-extend
            }
            lcn += off;
            runs.push((lcn, len));
        }
        o += ol;
    }
    runs
}

#[derive(Clone)]
struct Rec {
    parent: u32,
    name_off: u32,
    name_len: u16,
    rank: u8, // 0 best (Win32 name) .. 2 (DOS 8.3 only); 255 = no name yet
    used: bool,
    dir: bool,
    size: u64,
    mtime: i64,
}

impl Default for Rec {
    fn default() -> Self {
        Rec { parent: 0, name_off: 0, name_len: 0, rank: 255, used: false, dir: false, size: 0, mtime: 0 }
    }
}

struct Table {
    recs: Vec<Rec>,
    names: String,
}

fn ns_rank(ns: u8) -> u8 {
    match ns {
        1 | 3 => 0, // Win32, Win32+DOS
        0 => 1,     // POSIX
        _ => 2,     // DOS 8.3 only
    }
}

/// Parse one fixed-up record into the table.
fn parse(t: &mut Table, idx: usize, rec: &[u8]) {
    let flags = u16le(rec, 0x16);
    if flags & 1 == 0 {
        return; // not in use
    }
    let base = (u64le(rec, 0x20) & REF_MASK) as usize;
    let target = if base != 0 { base } else { idx };
    if target >= t.recs.len() {
        return;
    }
    if base == 0 {
        t.recs[target].used = true;
        t.recs[target].dir = flags & 2 != 0;
    }
    let mut o = u16le(rec, 0x14) as usize;
    while o + 16 <= rec.len() {
        let ty = u32le(rec, o);
        if ty == ATTR_END {
            break;
        }
        let len = u32le(rec, o + 4) as usize;
        if len < 16 || o + len > rec.len() {
            break;
        }
        let non_res = rec[o + 8] != 0;
        let name_len = rec[o + 9];
        let a = &rec[o..o + len];
        match ty {
            ATTR_STANDARD_INFO if !non_res && len >= 0x18 => {
                let vo = u16le(a, 0x14) as usize;
                if vo + 16 <= a.len() {
                    t.recs[target].mtime = ft_to_unix(u64le(a, vo + 8));
                }
            }
            ATTR_FILE_NAME if !non_res && len >= 0x18 => {
                let vo = u16le(a, 0x14) as usize;
                if vo + 0x42 <= a.len() {
                    let v = &a[vo..];
                    let parent = (u64le(v, 0) & REF_MASK) as u32;
                    let nlen = v[0x40] as usize;
                    let rank = ns_rank(v[0x41]);
                    if 0x42 + nlen * 2 <= v.len() && rank < t.recs[target].rank {
                        let u: Vec<u16> = (0..nlen).map(|i| u16le(v, 0x42 + i * 2)).collect();
                        let s = String::from_utf16_lossy(&u);
                        let r = &mut t.recs[target];
                        r.parent = parent;
                        r.rank = rank;
                        r.name_off = t.names.len() as u32;
                        r.name_len = s.len() as u16;
                        t.names.push_str(&s);
                    }
                }
            }
            ATTR_DATA if name_len == 0 => {
                if non_res {
                    if len >= 0x38 && u64le(a, 0x10) == 0 {
                        t.recs[target].size = u64le(a, 0x30); // real size, first extent only
                    }
                } else if len >= 0x18 {
                    t.recs[target].size = u32le(a, 0x10) as u64;
                }
            }
            _ => {}
        }
        o += len;
    }
}

pub fn scan(root: &str, prog: &Progress) -> Result<Dir, String> {
    let letter = root
        .chars()
        .next()
        .filter(|c| c.is_ascii_alphabetic())
        .ok_or("bad drive")?
        .to_ascii_uppercase();
    let mut f = File::open(format!(r"\\.\{letter}:")).map_err(|e| match e.raw_os_error() {
        Some(5) => "Fast scan needs admin rights".to_string(),
        _ => format!("Can't open {letter}: ({e})"),
    })?;

    let mut boot = [0u8; 512];
    f.read_exact(&mut boot).map_err(|e| e.to_string())?;
    if &boot[3..11] != b"NTFS    " {
        return Err(format!("{letter}: is not NTFS"));
    }
    let sector = u16le(&boot, 0x0B) as u64;
    let spc_raw = boot[0x0D] as u64;
    let spc = if spc_raw > 0x80 { 1u64 << (256 - spc_raw) } else { spc_raw };
    let cluster = sector * spc;
    let mft_lcn = u64le(&boot, 0x30);
    let cpr = boot[0x40] as i8;
    let rec_size = if cpr > 0 { cpr as u64 * cluster } else { 1u64 << (-(cpr as i32)) as u32 } as usize;
    if sector == 0 || cluster == 0 || rec_size < 512 {
        return Err("Unexpected NTFS layout".into());
    }

    // Record 0 is $MFT itself: its $DATA run list says where the whole table lives.
    let first_read = (rec_size as u64).max(cluster) as usize;
    let mut r0 = vec![0u8; first_read];
    f.seek(SeekFrom::Start(mft_lcn * cluster)).map_err(|e| e.to_string())?;
    f.read_exact(&mut r0).map_err(|e| e.to_string())?;
    let r0 = &mut r0[..rec_size];
    if !fixup(r0, sector as usize) {
        return Err("Couldn't read the MFT header".into());
    }
    let (mut runs, mut mft_bytes) = (Vec::new(), 0u64);
    let mut o = u16le(r0, 0x14) as usize;
    while o + 16 <= r0.len() {
        let ty = u32le(r0, o);
        let len = u32le(r0, o + 4) as usize;
        if ty == ATTR_END || len < 16 || o + len > r0.len() {
            break;
        }
        if ty == ATTR_DATA && r0[o + 8] != 0 && r0[o + 9] == 0 {
            let a = &r0[o..o + len];
            mft_bytes = u64le(a, 0x30);
            runs = data_runs(a, u16le(a, 0x20) as usize);
            break;
        }
        o += len;
    }
    if runs.is_empty() {
        return Err("Couldn't find the MFT's data".into());
    }
    let total = (mft_bytes / rec_size as u64) as usize;
    prog.total_records.store(total as u64, Ordering::Relaxed);

    let mut t = Table { recs: vec![Rec::default(); total], names: String::with_capacity(total * 16) };
    let mut buf = vec![0u8; CHUNK];
    let mut vbyte: u64 = 0; // byte offset inside $MFT
    for (lcn, clusters) in runs {
        let run_bytes = clusters * cluster;
        if lcn < 0 {
            vbyte += run_bytes;
            continue;
        }
        let mut done = 0u64;
        f.seek(SeekFrom::Start(lcn as u64 * cluster)).map_err(|e| e.to_string())?;
        while done < run_bytes {
            let n = (run_bytes - done).min(CHUNK as u64) as usize;
            f.read_exact(&mut buf[..n]).map_err(|e| e.to_string())?;
            for (k, chunk) in buf[..n].chunks_exact_mut(rec_size).enumerate() {
                let idx = ((vbyte + done) as usize + k * rec_size) / rec_size;
                if idx >= total {
                    break;
                }
                if fixup(chunk, sector as usize) {
                    parse(&mut t, idx, chunk);
                }
            }
            done += n as u64;
            prog.records.store(((vbyte + done) / rec_size as u64).min(total as u64), Ordering::Relaxed);
        }
        vbyte += run_bytes;
    }

    // children lists (compact: one array + offsets)
    let n = t.recs.len();
    if n <= ROOT || !t.recs[ROOT].used {
        return Err("Root folder not found in MFT".into());
    }
    let ok = |i: usize, r: &Rec| r.used && r.rank != 255 && i != ROOT;
    let mut start = vec![0u32; n + 1];
    for (i, r) in t.recs.iter().enumerate() {
        let p = r.parent as usize;
        if ok(i, r) && p < n && t.recs[p].used && t.recs[p].dir {
            start[p + 1] += 1;
        }
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut fill = start.clone();
    let mut kids = vec![0u32; start[n] as usize];
    for (i, r) in t.recs.iter().enumerate() {
        let p = r.parent as usize;
        if ok(i, r) && p < n && t.recs[p].used && t.recs[p].dir {
            kids[fill[p] as usize] = i as u32;
            fill[p] += 1;
        }
    }
    drop(fill);

    let ctx = Ctx { t: &t, start: &start, kids: &kids, prog };
    let root_path = format!("{letter}:\\");
    Ok(build(&ctx, ROOT, format!("{letter}:"), root_path, 0))
}

struct Ctx<'a> {
    t: &'a Table,
    start: &'a [u32],
    kids: &'a [u32],
    prog: &'a Progress,
}

fn name_of<'a>(t: &'a Table, r: &Rec) -> &'a str {
    let s = r.name_off as usize;
    &t.names[s..s + r.name_len as usize]
}

fn build(c: &Ctx, i: usize, name: String, path: String, depth: u32) -> Dir {
    let mut d = Dir {
        name, path, size: 0, files: 0, dirs: 0, newest: 0, small_bytes: 0, small_count: 0,
        denied: false, children: Vec::new(), big: Vec::new(),
    };
    c.prog.dirs.fetch_add(1, Ordering::Relaxed);
    let ks = &c.kids[c.start[i] as usize..c.start[i + 1] as usize];
    let mut sub = Vec::new();
    for &k in ks {
        let r = &c.t.recs[k as usize];
        if r.dir {
            if depth < 512 {
                sub.push(k as usize);
            }
        } else {
            d.files += 1;
            d.size += r.size;
            if r.mtime > d.newest {
                d.newest = r.mtime;
            }
            if r.size >= BIG_FILE {
                d.big.push(FileEnt { name: name_of(c.t, r).to_string(), size: r.size, modified: r.mtime });
            } else {
                d.small_bytes += r.size;
                d.small_count += 1;
            }
        }
    }
    c.prog.files.fetch_add(d.files, Ordering::Relaxed);
    c.prog.bytes.fetch_add(d.size, Ordering::Relaxed);
    let base = if d.path.ends_with('\\') { d.path.clone() } else { format!("{}\\", d.path) };
    d.children = sub
        .into_par_iter()
        .map(|k| {
            let nm = name_of(c.t, &c.t.recs[k]).to_string();
            let p = format!("{base}{nm}");
            build(c, k, nm, p, depth + 1)
        })
        .collect();
    for ch in &d.children {
        d.size += ch.size;
        d.files += ch.files;
        d.dirs += ch.dirs + 1;
        if ch.newest > d.newest {
            d.newest = ch.newest;
        }
    }
    d
}
