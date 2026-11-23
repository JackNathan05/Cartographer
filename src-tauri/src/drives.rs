//! Drive list via Win32 (letter, label, type, size, free).

use serde::Serialize;
use std::ptr::null_mut;
use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};

#[derive(Serialize, Clone)]
pub struct Drive {
    pub letter: String,
    pub label: String,
    pub kind: &'static str,
    pub fs: String,
    pub total: i64,
    pub free: i64,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(b: &[u16]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..n])
}

/// Asking Windows about every drive takes ~0.6 s here (network / cloud drives answer slowly), and one
/// report can ask several times, so answers are reused for a few seconds.
pub fn list() -> Vec<Drive> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, Vec<Drive>)>> = Mutex::new(None);
    if let Ok(c) = CACHE.lock() {
        if let Some((t, v)) = c.as_ref() {
            if t.elapsed() < Duration::from_secs(5) {
                return v.clone();
            }
        }
    }
    let v = list_now();
    if let Ok(mut c) = CACHE.lock() {
        *c = Some((Instant::now(), v.clone()));
    }
    v
}

fn list_now() -> Vec<Drive> {
    let mut out = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = char::from(b'A' + i as u8).to_string();
        let root = wide(&format!("{letter}:\\"));
        let kind = match unsafe { GetDriveTypeW(root.as_ptr()) } {
            2 => "removable",
            3 => "fixed",
            4 => "network",
            5 => "cdrom",
            6 => "ramdisk",
            _ => continue,
        };
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        if unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut avail, &mut total, &mut free) } == 0 {
            continue; // e.g. empty card reader
        }
        let mut label = [0u16; 261];
        let mut fsname = [0u16; 261];
        unsafe {
            GetVolumeInformationW(
                root.as_ptr(), label.as_mut_ptr(), label.len() as u32, null_mut(), null_mut(), null_mut(),
                fsname.as_mut_ptr(), fsname.len() as u32,
            )
        };
        out.push(Drive {
            letter,
            label: from_wide(&label),
            kind,
            fs: from_wide(&fsname),
            total: total as i64,
            free: free as i64,
        });
    }
    out
}

/// Used bytes on the drive that `root` (e.g. "C:\\") lives on.
pub fn used(root: &str) -> Option<i64> {
    let l = root.chars().next()?.to_ascii_uppercase().to_string();
    list().into_iter().find(|d| d.letter == l).map(|d| d.total - d.free)
}
