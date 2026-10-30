//! Auto-update. The newest GitHub Release of `globalanomalyindex/redraw` carries a small
//! `latest.txt` (version, SHA-256, download address) next to `redraw.exe`. The overlay checks it
//! at launch and every 30 minutes, on a background thread. A newer version is downloaded, checked
//! (size, SHA-256, and that it really is a 64-bit Windows program) and swapped in by renaming:
//! Windows lets a running exe be renamed but not overwritten, so the running copy keeps going,
//! the new one is on disk under the real name, and the next launch is the new version. The old
//! exe stays as `redraw.exe.old`.
//!
//! This file is pure logic plus file renames. The network is a function handed in by the caller,
//! so every path below is tested without one. Only github.com release addresses are accepted.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Where this program checks for itself.
pub const MANIFEST_URL: &str = "https://github.com/globalanomalyindex/redraw/releases/latest/download/latest.txt";
/// The only place a download may come from.
const RELEASES: &str = "https://github.com/globalanomalyindex/redraw/releases/download/";
/// The version this build is (`REDRAW_VERSION` from the release workflow, else Cargo.toml's).
pub const VERSION: &str = env!("REDRAW_VERSION");
pub const CHECK_EVERY: Duration = Duration::from_secs(30 * 60);

const MAX_EXE: usize = 16 * 1024 * 1024;
const MIN_EXE: usize = 100 * 1024;
const MAX_MANIFEST: usize = 4096;

/// Set while the two renames of an install run, so a quit can wait for them instead of leaving
/// the exe half swapped.
static SWAPPING: AtomicBool = AtomicBool::new(false);

/// Where to read `latest.txt` from: GitHub, except in test builds that name a local server.
pub fn manifest_url() -> String {
    #[cfg(feature = "local-update")]
    if let Ok(base) = std::env::var("REDRAW_UPDATE_BASE") {
        return format!("{base}/latest.txt");
    }
    MANIFEST_URL.to_string()
}

/// A dotted version such as `0.2.7`, compared number by number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version(Vec<u32>);

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let t = text.trim().trim_start_matches('v');
        let parts: Option<Vec<u32>> = t.split('.').map(|p| if p.is_empty() { None } else { p.parse().ok() }).collect();
        parts.filter(|p| !p.is_empty() && p.len() <= 4).map(Version)
    }

    pub fn is_newer_than(&self, other: &Version) -> bool {
        let n = self.0.len().max(other.0.len());
        for i in 0..n {
            let (a, b) = (self.0.get(i).copied().unwrap_or(0), other.0.get(i).copied().unwrap_or(0));
            if a != b {
                return a > b;
            }
        }
        false
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let parts: Vec<String> = self.0.iter().map(u32::to_string).collect();
        write!(f, "{}", parts.join("."))
    }
}

/// What `latest.txt` says: three `key=value` lines (and an optional `size=`).
#[derive(Debug, PartialEq, Eq)]
pub struct Manifest {
    pub version: Version,
    pub sha256: [u8; 32],
    pub url: String,
    pub size: Option<usize>,
}

/// A download address we are willing to follow.
fn allowed_url(url: &str) -> bool {
    let base_ok = url.starts_with(RELEASES) || (cfg!(feature = "local-update") && url.starts_with("http://127.0.0.1:"));
    base_ok && url.len() < 300 && url.ends_with("/redraw.exe") && !url.contains("..") && url.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._:-".contains(&b))
}

fn hex32(text: &str) -> Option<[u8; 32]> {
    let t = text.trim();
    if t.len() != 64 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&t[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

pub fn parse_manifest(text: &str) -> Option<Manifest> {
    let (mut version, mut sha, mut url, mut size) = (None, None, None, None);
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k.trim() {
            "version" => version = Version::parse(v),
            "sha256" => sha = hex32(v),
            "url" => url = Some(v.trim().to_string()).filter(|u| allowed_url(u)),
            "size" => size = v.trim().parse().ok().filter(|s| (MIN_EXE..=MAX_EXE).contains(s)),
            _ => {}
        }
    }
    Some(Manifest { version: version?, sha256: sha?, url: url?, size })
}

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([block[i * 4], block[i * 4 + 1], block[i * 4 + 2], block[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Is this plausibly a Windows program for this machine's architecture? (Not a guarantee of anything:
/// the SHA-256 is what says it is the file that was published.)
pub fn looks_like_exe(bytes: &[u8]) -> bool {
    if bytes.len() < MIN_EXE || bytes.len() > MAX_EXE || &bytes[..2] != b"MZ" {
        return false;
    }
    let pe = u32::from_le_bytes([bytes[0x3C], bytes[0x3D], bytes[0x3E], bytes[0x3F]]) as usize;
    if pe + 6 > bytes.len() || &bytes[pe..pe + 4] != b"PE\0\0" {
        return false;
    }
    let machine = u16::from_le_bytes([bytes[pe + 4], bytes[pe + 5]]);
    machine == if cfg!(target_arch = "aarch64") { 0xAA64 } else { 0x8664 }
}

fn beside(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".");
    name.push(suffix);
    exe.with_file_name(name)
}

/// Put `bytes` in place of `exe` (which may be running): write beside it, rename the old one to
/// `.old`, rename the new one into place, and undo it all if the last step fails.
pub fn install(exe: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let (new, old) = (beside(exe, "new"), beside(exe, "old"));
    std::fs::write(&new, bytes)?;
    SWAPPING.store(true, Ordering::SeqCst);
    let result = (|| {
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old)?;
        if let Err(e) = std::fs::rename(&new, exe) {
            let _ = std::fs::rename(&old, exe);
            return Err(e);
        }
        Ok(())
    })();
    SWAPPING.store(false, Ordering::SeqCst);
    if result.is_err() {
        let _ = std::fs::remove_file(&new);
    }
    result
}

/// Wait (at most `max`) for an install's renames to finish, so quitting never leaves the exe half swapped.
pub fn wait_for_swap(max: Duration) {
    let start = Instant::now();
    while SWAPPING.load(Ordering::SeqCst) && start.elapsed() < max {
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// At launch: a download that was cut off leaves `redraw.exe.new` behind; it is never used, so drop it.
pub fn clean_leftovers(exe: &Path) {
    let _ = std::fs::remove_file(beside(exe, "new"));
}

/// How a check ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    UpToDate,
    /// a newer version is on disk, and runs from the next launch
    Installed(String),
    Failed(String),
}

/// Fetches an address, reading at most this many bytes.
pub type Fetch<'a> = &'a dyn Fn(&str, usize) -> Result<Vec<u8>, String>;

/// One full check: read the manifest, and if it names a newer version, download, verify and install it.
pub fn check_and_install(exe: &Path, current: &Version, manifest_url: &str, fetch: Fetch) -> Outcome {
    let manifest = match fetch(manifest_url, MAX_MANIFEST).map_err(|e| format!("cannot reach GitHub: {e}")) {
        Ok(b) => b,
        Err(e) => return Outcome::Failed(e),
    };
    let Some(m) = std::str::from_utf8(&manifest).ok().and_then(parse_manifest) else { return Outcome::Failed("latest.txt is not understood".to_string()) };
    if !m.version.is_newer_than(current) {
        return Outcome::UpToDate;
    }
    let bytes = match fetch(&m.url, MAX_EXE) {
        Ok(b) => b,
        Err(e) => return Outcome::Failed(format!("download failed: {e}")),
    };
    if m.size.is_some_and(|s| s != bytes.len()) {
        return Outcome::Failed("download is the wrong size".to_string());
    }
    if sha256(&bytes) != m.sha256 {
        return Outcome::Failed("download does not match its checksum".to_string());
    }
    if !looks_like_exe(&bytes) {
        return Outcome::Failed("download is not a Windows program".to_string());
    }
    match install(exe, &bytes) {
        Ok(()) => Outcome::Installed(m.version.to_string()),
        Err(e) => Outcome::Failed(format!("cannot replace the program: {e}")),
    }
}

/// What the title area says about updates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Checking,
    UpToDate,
    Installed(String),
    Failed,
    Off,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// A made-up but valid-looking 64-bit exe of a recognisable size.
    fn fake_exe(tag: u8) -> Vec<u8> {
        let mut b = vec![tag; 200 * 1024];
        b[0] = b'M';
        b[1] = b'Z';
        b[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        let machine: u16 = if cfg!(target_arch = "aarch64") { 0xAA64 } else { 0x8664 };
        b[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        b
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("redraw-update-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn manifest_for(version: &str, exe: &[u8]) -> String {
        format!("version={version}\nsha256={}\nsize={}\nurl={RELEASES}v{version}/redraw.exe\n", hex(&sha256(exe)), exe.len())
    }

    /// A fetcher backed by a map of address -> bytes, counting what it was asked for.
    fn server(files: Vec<(String, Vec<u8>)>) -> (impl Fn(&str, usize) -> Result<Vec<u8>, String>, std::rc::Rc<RefCell<Vec<String>>>) {
        let map: HashMap<String, Vec<u8>> = files.into_iter().collect();
        let asked = std::rc::Rc::new(RefCell::new(Vec::new()));
        let log = asked.clone();
        (
            move |url: &str, max: usize| {
                log.borrow_mut().push(url.to_string());
                match map.get(url) {
                    Some(b) if b.len() <= max => Ok(b.clone()),
                    Some(_) => Err("too large".to_string()),
                    None => Err("404".to_string()),
                }
            },
            asked,
        )
    }

    #[test]
    fn sha256_matches_the_published_test_vectors() {
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hex(&sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        assert_eq!(hex(&sha256(&vec![b'a'; 1_000_000])), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
        for len in [55, 56, 57, 63, 64, 65, 119, 120, 128] {
            assert_eq!(sha256(&vec![7u8; len]), sha256(&vec![7u8; len]), "padding edge {len}");
        }
    }

    #[test]
    fn versions_compare_number_by_number() {
        let v = |s: &str| Version::parse(s).unwrap();
        assert!(v("0.2.10").is_newer_than(&v("0.2.9")), "10 is more than 9, not less as text");
        assert!(v("1.0").is_newer_than(&v("0.9.99")));
        assert!(!v("0.2.0").is_newer_than(&v("0.2.0")));
        assert!(!v("1.0").is_newer_than(&v("1.0.0")), "missing parts count as zero");
        assert!(!v("0.1.5").is_newer_than(&v("0.2.0")));
        assert_eq!(v("v0.2.7").to_string(), "0.2.7");
        for bad in ["", "v", "1..2", "1.x", "1.2.3.4.5", "-1", "0.2.7-beta"] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
        assert!(Version::parse(VERSION).is_some(), "this build's own version parses: {VERSION}");
    }

    #[test]
    fn the_manifest_is_read_strictly() {
        let exe = fake_exe(1);
        let good = manifest_for("0.2.7", &exe);
        let m = parse_manifest(&good).expect("good manifest");
        assert_eq!((m.version.to_string(), m.size, m.sha256), ("0.2.7".to_string(), Some(exe.len()), sha256(&exe)));
        assert!(m.url.starts_with(RELEASES) && m.url.ends_with("/redraw.exe"));
        let sha = hex(&sha256(&exe));
        for (what, bad) in [
            ("no version", format!("sha256={sha}\nurl={RELEASES}v1/redraw.exe\n")),
            ("no hash", format!("version=1.0\nurl={RELEASES}v1/redraw.exe\n")),
            ("short hash", format!("version=1.0\nsha256={}\nurl={RELEASES}v1/redraw.exe\n", &sha[..60])),
            ("no url", format!("version=1.0\nsha256={sha}\n")),
            ("another host", format!("version=1.0\nsha256={sha}\nurl=https://evil.example/globalanomalyindex/redraw/releases/download/v1/redraw.exe\n")),
            ("another repo", format!("version=1.0\nsha256={sha}\nurl=https://github.com/someone/else/releases/download/v1/redraw.exe\n")),
            ("plain http", format!("version=1.0\nsha256={sha}\nurl=http://github.com/globalanomalyindex/redraw/releases/download/v1/redraw.exe\n")),
            ("path trick", format!("version=1.0\nsha256={sha}\nurl={RELEASES}../../../../x/redraw.exe\n")),
            ("another file", format!("version=1.0\nsha256={sha}\nurl={RELEASES}v1/setup.exe\n")),
            ("odd characters", format!("version=1.0\nsha256={sha}\nurl={RELEASES}v1/re draw?x=1/redraw.exe\n")),
            ("not text at all", "\u{0}\u{1}\u{2}".to_string()),
        ] {
            assert_eq!(parse_manifest(&bad), None, "{what}");
        }
        assert!(parse_manifest(&format!("{good}size=12\n")).is_some_and(|m| m.size.is_none()), "an absurd size is ignored, not trusted");
    }

    #[test]
    fn only_plausible_windows_programs_pass_the_header_check() {
        assert!(looks_like_exe(&fake_exe(1)));
        assert!(!looks_like_exe(&[]));
        assert!(!looks_like_exe(&vec![0u8; 300 * 1024]), "no MZ");
        let mut no_pe = fake_exe(1);
        no_pe[0x80] = b'X';
        assert!(!looks_like_exe(&no_pe));
        let mut wrong_arch = fake_exe(1);
        wrong_arch[0x84..0x86].copy_from_slice(&0x014Cu16.to_le_bytes());
        assert!(!looks_like_exe(&wrong_arch), "a 32-bit program is not this one");
        let mut far = fake_exe(1);
        far[0x3C..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(!looks_like_exe(&far), "a header offset past the end does not panic");
        assert!(!looks_like_exe(&fake_exe(1)[..50 * 1024]), "too small");
    }

    #[test]
    fn a_newer_release_is_downloaded_verified_and_swapped_in_keeping_the_old_one() {
        let dir = temp_dir("swap");
        let exe = dir.join("redraw.exe");
        std::fs::write(&exe, fake_exe(1)).unwrap();
        let new = fake_exe(2);
        let (fetch, asked) = server(vec![(MANIFEST_URL.to_string(), manifest_for("0.2.7", &new).into_bytes()), (format!("{RELEASES}v0.2.7/redraw.exe"), new.clone())]);
        let out = check_and_install(&exe, &Version::parse("0.2.0").unwrap(), MANIFEST_URL, &fetch);
        assert_eq!(out, Outcome::Installed("0.2.7".to_string()));
        assert_eq!(std::fs::read(&exe).unwrap(), new, "the real name now holds the new version");
        assert_eq!(std::fs::read(beside(&exe, "old")).unwrap(), fake_exe(1), "the previous version is kept");
        assert!(!beside(&exe, "new").exists(), "no staging file left");
        assert_eq!(asked.borrow().len(), 2, "manifest, then the exe");
        // Installing the next one replaces the backup, not piles up files.
        let newer = fake_exe(3);
        let (fetch, _) = server(vec![(MANIFEST_URL.to_string(), manifest_for("0.2.8", &newer).into_bytes()), (format!("{RELEASES}v0.2.8/redraw.exe"), newer.clone())]);
        assert_eq!(check_and_install(&exe, &Version::parse("0.2.7").unwrap(), MANIFEST_URL, &fetch), Outcome::Installed("0.2.8".to_string()));
        assert_eq!(std::fs::read(beside(&exe, "old")).unwrap(), new);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "just the exe and one backup");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_is_downloaded_when_already_current_or_ahead() {
        let dir = temp_dir("current");
        let exe = dir.join("redraw.exe");
        std::fs::write(&exe, fake_exe(1)).unwrap();
        let other = fake_exe(2);
        for (published, mine) in [("0.2.0", "0.2.0"), ("0.1.9", "0.2.0"), ("0.2.0", "0.2.5")] {
            let (fetch, asked) = server(vec![(MANIFEST_URL.to_string(), manifest_for(published, &other).into_bytes())]);
            assert_eq!(check_and_install(&exe, &Version::parse(mine).unwrap(), MANIFEST_URL, &fetch), Outcome::UpToDate, "{published} vs mine {mine}");
            assert_eq!(asked.borrow().len(), 1, "only the small manifest was read");
        }
        assert_eq!(std::fs::read(&exe).unwrap(), fake_exe(1), "and the program was not touched (no downgrade)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_download_never_replaces_the_program() {
        let dir = temp_dir("bad");
        let exe = dir.join("redraw.exe");
        std::fs::write(&exe, fake_exe(1)).unwrap();
        let good = fake_exe(2);
        let url = format!("{RELEASES}v0.2.7/redraw.exe");
        let mut tampered = good.clone();
        tampered[5000] ^= 0xFF;
        let mut not_exe = vec![9u8; 200 * 1024];
        not_exe[0] = b'X';
        let cases: Vec<(&str, Vec<(String, Vec<u8>)>)> = vec![
            ("manifest unreachable", vec![]),
            ("manifest garbage", vec![(MANIFEST_URL.to_string(), b"<html>rate limited</html>".to_vec())]),
            ("exe missing", vec![(MANIFEST_URL.to_string(), manifest_for("0.2.7", &good).into_bytes())]),
            ("exe tampered", vec![(MANIFEST_URL.to_string(), manifest_for("0.2.7", &good).into_bytes()), (url.clone(), tampered)]),
            ("exe truncated", vec![(MANIFEST_URL.to_string(), manifest_for("0.2.7", &good).into_bytes()), (url.clone(), good[..150 * 1024].to_vec())]),
            ("hash matches but it is not a program", vec![(MANIFEST_URL.to_string(), manifest_for("0.2.7", &not_exe).into_bytes()), (url.clone(), not_exe.clone())]),
        ];
        for (what, files) in cases {
            let (fetch, _) = server(files);
            let out = check_and_install(&exe, &Version::parse("0.2.0").unwrap(), MANIFEST_URL, &fetch);
            assert!(matches!(out, Outcome::Failed(_)), "{what}: {out:?}");
            assert_eq!(std::fs::read(&exe).unwrap(), fake_exe(1), "{what}: the program is untouched");
            assert!(!beside(&exe, "new").exists() && !beside(&exe, "old").exists(), "{what}: nothing left behind");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_swap_is_undone_and_leftovers_are_cleaned() {
        let dir = temp_dir("rollback");
        let exe = dir.join("redraw.exe");
        // No exe to rename: the swap fails at the first rename, and nothing is left staged.
        assert!(install(&exe, &fake_exe(2)).is_err());
        assert!(!beside(&exe, "new").exists());
        std::fs::write(&exe, fake_exe(1)).unwrap();
        std::fs::write(beside(&exe, "new"), b"half a download").unwrap();
        clean_leftovers(&exe);
        assert!(!beside(&exe, "new").exists() && exe.exists(), "only the staging file goes");
        wait_for_swap(Duration::from_millis(50));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
