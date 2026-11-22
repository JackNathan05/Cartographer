//! Fast parallel folder walk (v0.1 "no admin" scanner).
//! Directory sizes are computed bottom-up. Files >= BIG_FILE are kept individually;
//! smaller files are summed per folder so the index stays small.
//! Junctions / symlinked folders are skipped so nothing is counted twice.

use rayon::prelude::*;
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
pub const BIG_FILE: u64 = 1024 * 1024; // 1 MB

#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub denied: AtomicU64,
}

pub struct FileEnt {
    pub name: String,
    pub size: u64,
    pub modified: i64,
}

pub struct Dir {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub files: u64,
    pub dirs: u64,
    pub newest: i64,
    pub small_bytes: u64,
    pub small_count: u64,
    pub denied: bool,
    pub children: Vec<Dir>,
    pub big: Vec<FileEnt>,
}

fn mtime(md: &fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn scan(path: &Path, name: String, prog: &Progress) -> Dir {
    let mut d = Dir {
        name,
        path: path.to_string_lossy().into_owned(),
        size: 0,
        files: 0,
        dirs: 0,
        newest: 0,
        small_bytes: 0,
        small_count: 0,
        denied: false,
        children: Vec::new(),
        big: Vec::new(),
    };
    prog.dirs.fetch_add(1, Ordering::Relaxed);

    let rd = match fs::read_dir(path) {
        Ok(r) => r,
        Err(_) => {
            d.denied = true;
            prog.denied.fetch_add(1, Ordering::Relaxed);
            return d;
        }
    };

    let mut subdirs = Vec::new();
    let mut bytes_here = 0u64;
    for e in rd.flatten() {
        // On Windows this metadata comes from the directory listing itself (no extra file open).
        let md = match e.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if md.is_dir() {
            if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                continue; // junction / symlink: its target is counted where it really lives
            }
            subdirs.push((e.path(), e.file_name().to_string_lossy().into_owned()));
        } else {
            let size = md.len();
            let m = mtime(&md);
            d.files += 1;
            d.size += size;
            bytes_here += size;
            if m > d.newest {
                d.newest = m;
            }
            if size >= BIG_FILE {
                d.big.push(FileEnt { name: e.file_name().to_string_lossy().into_owned(), size, modified: m });
            } else {
                d.small_bytes += size;
                d.small_count += 1;
            }
        }
    }
    prog.files.fetch_add(d.files, Ordering::Relaxed);
    prog.bytes.fetch_add(bytes_here, Ordering::Relaxed);

    d.children = subdirs
        .into_par_iter()
        .map(|(p, n)| scan(&p, n, prog))
        .collect();

    for c in &d.children {
        d.size += c.size;
        d.files += c.files;
        d.dirs += c.dirs + 1;
        if c.newest > d.newest {
            d.newest = c.newest;
        }
    }
    d
}
