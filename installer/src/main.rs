#![windows_subsystem = "windows"]
//! reDraw Setup: one file that installs reDraw for the current user (no administrator).
//!
//! It puts `redraw.exe` in `%LOCALAPPDATA%\reDraw`, adds a Start menu shortcut and an entry in
//! Settings > Apps, and (only if asked) starts reDraw when you sign in. It keeps a copy of itself
//! there as `uninstall.exe`; run with `--uninstall` it removes exactly what it added. reDraw updates
//! itself in that folder, so no administrator rights are ever needed.
//!
//! For testing and scripting: `--silent` (no questions), `--dir <folder>`, `--no-shortcut`,
//! `--no-registry`, `--no-launch`, `--startup` / `--no-startup`, and for removal `--purge` (also
//! delete the settings file).

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// The program being installed, built into this file.
static EXE: &[u8] = include_bytes!(env!("REDRAW_EXE"));
const VERSION: &str = match option_env!("REDRAW_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
const REPO_URL: &str = "https://github.com/globalanomalyindex/redraw";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\reDraw";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "reDraw";
const MARKER: &str = "redraw.install";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, PartialEq)]
struct Opts {
    silent: bool,
    uninstall: bool,
    dir: Option<PathBuf>,
    shortcut: bool,
    registry: bool,
    launch: bool,
    /// `None` = ask (or no, when silent)
    startup: Option<bool>,
    purge: bool,
}

fn parse_args(args: &[String]) -> Opts {
    let mut o = Opts { silent: false, uninstall: false, dir: None, shortcut: true, registry: true, launch: true, startup: None, purge: false };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.to_ascii_lowercase().as_str() {
            "--silent" | "/s" => o.silent = true,
            "--uninstall" => o.uninstall = true,
            "--dir" => o.dir = it.next().map(PathBuf::from),
            "--no-shortcut" => o.shortcut = false,
            "--no-registry" => o.registry = false,
            "--no-launch" => o.launch = false,
            "--startup" => o.startup = Some(true),
            "--no-startup" => o.startup = Some(false),
            "--purge" => o.purge = true,
            _ => {}
        }
    }
    o
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn local_app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Users\Public\AppData\Local"))
}

fn default_dir() -> PathBuf {
    local_app_data().join("reDraw")
}

fn shortcut_path() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join(r"Microsoft\Windows\Start Menu\Programs\reDraw.lnk")
}

/// A path for comparing: no `\\?\` prefix, lower case, one kind of slash.
fn norm(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', r"\");
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_ascii_lowercase()
}

/// Text inside a single-quoted PowerShell string.
fn ps_quote(s: &str) -> String {
    s.replace('\'', "''")
}

/// Ask a yes/no question. `default_yes` picks which button Enter presses.
fn ask(text: &str, default_yes: bool) -> bool {
    let flags = MB_YESNO | MB_ICONQUESTION | if default_yes { MB_DEFBUTTON1 } else { MB_DEFBUTTON2 };
    unsafe { MessageBoxW(null_mut(), wide(text).as_ptr(), wide("reDraw Setup").as_ptr(), flags) == IDYES }
}

fn tell(text: &str, error: bool) {
    unsafe { MessageBoxW(null_mut(), wide(text).as_ptr(), wide("reDraw Setup").as_ptr(), MB_OK | if error { MB_ICONERROR } else { MB_ICONINFORMATION }) };
}

// --- registry ----------------------------------------------------------------------------------

unsafe fn reg_set(subkey: &str, name: &str, value: &str) -> bool {
    let mut key: HKEY = null_mut();
    if RegCreateKeyExW(HKEY_CURRENT_USER, wide(subkey).as_ptr(), 0, null(), 0, KEY_WRITE, null(), &mut key, null_mut()) != 0 {
        return false;
    }
    let v = wide(value);
    let ok = RegSetValueExW(key, wide(name).as_ptr(), 0, REG_SZ, v.as_ptr() as *const u8, (v.len() * 2) as u32) == 0;
    RegCloseKey(key);
    ok
}

unsafe fn reg_set_dword(subkey: &str, name: &str, value: u32) {
    let mut key: HKEY = null_mut();
    if RegCreateKeyExW(HKEY_CURRENT_USER, wide(subkey).as_ptr(), 0, null(), 0, KEY_WRITE, null(), &mut key, null_mut()) == 0 {
        RegSetValueExW(key, wide(name).as_ptr(), 0, REG_DWORD, &value as *const u32 as *const u8, 4);
        RegCloseKey(key);
    }
}

unsafe fn reg_delete_value(subkey: &str, name: &str) {
    let mut key: HKEY = null_mut();
    if RegOpenKeyExW(HKEY_CURRENT_USER, wide(subkey).as_ptr(), 0, KEY_WRITE, &mut key) == 0 {
        RegDeleteValueW(key, wide(name).as_ptr());
        RegCloseKey(key);
    }
}

// --- the running program -----------------------------------------------------------------------

/// Image path of the process that owns `hwnd`.
unsafe fn window_exe(hwnd: HWND) -> Option<PathBuf> {
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    let h = OpenProcess(0x1000, 0, pid); // PROCESS_QUERY_LIMITED_INFORMATION
    if h.is_null() {
        return None;
    }
    let mut buf = [0u16; 520];
    let mut len = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0;
    CloseHandle(h);
    ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

/// Overlay windows belonging to the `redraw.exe` in `dir` (and no other copy).
unsafe fn overlays_in(dir: &Path) -> Vec<HWND> {
    let want = norm(&dir.join("redraw.exe"));
    let (mut found, mut h) = (Vec::new(), null_mut());
    loop {
        h = FindWindowExW(null_mut(), h, wide("reDrawOverlay").as_ptr(), null());
        if h.is_null() {
            return found;
        }
        if window_exe(h).is_some_and(|p| norm(&p) == want) {
            found.push(h);
        }
    }
}

/// Ask the reDraw running from `dir` to close, and wait for it. Other copies are never touched.
fn close_running(dir: &Path) {
    unsafe {
        for h in overlays_in(dir) {
            PostMessageW(h, WM_CLOSE, 0, 0);
        }
        let start = Instant::now();
        while !overlays_in(dir).is_empty() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

// --- files -------------------------------------------------------------------------------------

/// Write `bytes` as `dir\redraw.exe`. A running exe cannot be overwritten but can be renamed, so the
/// old one is moved aside to `redraw.exe.old` first.
fn put_exe(dir: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let exe = dir.join("redraw.exe");
    if exe.exists() {
        let old = dir.join("redraw.exe.old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&exe, &old)?;
    }
    std::fs::write(&exe, bytes)?;
    Ok(exe)
}

fn make_shortcut(exe: &Path, dir: &Path) -> bool {
    let lnk = shortcut_path();
    if let Some(parent) = lnk.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.WorkingDirectory='{}';$s.Description='reDraw crosshair overlay';$s.Save()",
        ps_quote(&lnk.to_string_lossy()),
        ps_quote(&exe.to_string_lossy()),
        ps_quote(&dir.to_string_lossy())
    );
    std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .is_ok_and(|s| s.success())
        && lnk.exists()
}

// --- install -----------------------------------------------------------------------------------

fn install(o: &Opts) -> Result<String, String> {
    let dir = o.dir.clone().unwrap_or_else(default_dir);
    if !o.silent {
        let text = format!(
            "Install reDraw {VERSION}?\n\nIt installs for you only (no administrator needed) into:\n{}\n\nreDraw never reads game data. It checks {REPO_URL} for updates every 30 minutes; you can turn that off in its menu.",
            dir.display()
        );
        if !ask(&text, true) {
            return Ok(String::new());
        }
    }
    let startup = match o.startup {
        Some(v) => v,
        None => !o.silent && ask("Start reDraw when you sign in to Windows?", false),
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    close_running(&dir);
    let exe = put_exe(&dir, EXE).map_err(|e| format!("Could not write redraw.exe: {e}"))?;
    // The uninstaller is a copy of this very file.
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let uninstaller = dir.join("uninstall.exe");
    if norm(&me) != norm(&uninstaller) {
        let _ = std::fs::remove_file(&uninstaller);
        std::fs::copy(&me, &uninstaller).map_err(|e| format!("Could not write uninstall.exe: {e}"))?;
    }
    std::fs::write(dir.join(MARKER), format!("reDraw {VERSION}\n")).map_err(|e| e.to_string())?;
    if o.registry {
        unsafe {
            reg_set(UNINSTALL_KEY, "DisplayName", "reDraw");
            reg_set(UNINSTALL_KEY, "DisplayVersion", VERSION);
            reg_set(UNINSTALL_KEY, "Publisher", "Christopher Robin Fiore");
            reg_set(UNINSTALL_KEY, "InstallLocation", &dir.to_string_lossy());
            reg_set(UNINSTALL_KEY, "DisplayIcon", &exe.to_string_lossy());
            reg_set(UNINSTALL_KEY, "UninstallString", &format!("\"{}\" --uninstall", uninstaller.display()));
            reg_set(UNINSTALL_KEY, "URLInfoAbout", REPO_URL);
            reg_set_dword(UNINSTALL_KEY, "NoModify", 1);
            reg_set_dword(UNINSTALL_KEY, "NoRepair", 1);
            if startup {
                reg_set(RUN_KEY, RUN_VALUE, &format!("\"{}\"", exe.display()));
            } else {
                reg_delete_value(RUN_KEY, RUN_VALUE);
            }
        }
    }
    let shortcut_ok = !o.shortcut || make_shortcut(&exe, &dir);
    if o.launch {
        let _ = std::process::Command::new(&exe).current_dir(&dir).spawn();
    }
    let mut msg = format!("reDraw {VERSION} is installed in {}.", dir.display());
    if o.launch {
        msg.push_str("\n\nIt is running now: press F10 to open the menu.");
    }
    if !shortcut_ok {
        msg.push_str("\n\n(The Start menu shortcut could not be made; you can still run redraw.exe from that folder.)");
    }
    Ok(msg)
}

// --- uninstall ---------------------------------------------------------------------------------

fn uninstall(o: &Opts) -> Result<String, String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = o.dir.clone().or_else(|| me.parent().map(Path::to_path_buf)).ok_or("no install folder")?;
    // Only ever touch a folder this installer made.
    if !dir.join(MARKER).exists() {
        return Err(format!("{} is not a reDraw install folder (no {MARKER} in it), so nothing was removed.", dir.display()));
    }
    if !o.silent && !ask(&format!("Remove reDraw from {}?", dir.display()), true) {
        return Ok(String::new());
    }
    let purge = o.purge || (!o.silent && ask("Also delete your reDraw settings (redraw.cfg)?", false));
    close_running(&dir);
    if o.registry {
        unsafe {
            RegDeleteKeyW(HKEY_CURRENT_USER, wide(UNINSTALL_KEY).as_ptr());
            reg_delete_value(RUN_KEY, RUN_VALUE);
        }
    }
    if o.shortcut {
        let _ = std::fs::remove_file(shortcut_path());
    }
    // Only the files this installer and reDraw's updater make; never a recursive delete.
    let mut names = vec!["redraw.exe", "redraw.exe.old", "redraw.exe.new", MARKER];
    if purge {
        names.extend(["redraw.cfg", "redraw.cfg.tmp"]);
    }
    let mut stuck = Vec::new();
    for name in names {
        let p = dir.join(name);
        if p.exists() && std::fs::remove_file(&p).is_err() {
            // a program that is still closing can hold its exe for a moment
            std::thread::sleep(Duration::from_millis(700));
            if std::fs::remove_file(&p).is_err() {
                stuck.push(name);
            }
        }
    }
    // This file is running, so it cannot delete itself: a hidden command does it a moment after we exit.
    let me_here = me.parent().is_some_and(|p| norm(p) == norm(&dir));
    if me_here {
        // raw_arg: the command line goes to cmd.exe exactly as written (normal quoting would mangle the inner quotes)
        let cmd = format!("/C ping -n 3 127.0.0.1 >nul & del /f /q \"{}\" & rmdir \"{}\"", me.display(), dir.display());
        let _ = std::process::Command::new("cmd.exe").raw_arg(cmd).creation_flags(CREATE_NO_WINDOW).spawn();
    } else {
        let _ = std::fs::remove_file(dir.join("uninstall.exe"));
        let _ = std::fs::remove_dir(&dir);
    }
    if stuck.is_empty() {
        Ok("reDraw was removed.".to_string())
    } else {
        Err(format!("reDraw was mostly removed, but these could not be deleted (is it still running?): {}", stuck.join(", ")))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let o = parse_args(&args);
    let result = if o.uninstall { uninstall(&o) } else { install(&o) };
    match result {
        Ok(msg) if !msg.is_empty() && !o.silent => tell(&msg, false),
        Err(msg) => {
            if !o.silent {
                tell(&msg, true);
            }
            std::process::exit(1);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn arguments_set_only_what_they_name() {
        let d = parse_args(&args(&[]));
        assert_eq!(d, Opts { silent: false, uninstall: false, dir: None, shortcut: true, registry: true, launch: true, startup: None, purge: false });
        let o = parse_args(&args(&["--SILENT", "--dir", r"C:\Temp\x y", "--no-shortcut", "--no-registry", "--no-launch", "--startup", "--purge", "--uninstall"]));
        assert_eq!(o, Opts { silent: true, uninstall: true, dir: Some(PathBuf::from(r"C:\Temp\x y")), shortcut: false, registry: false, launch: false, startup: Some(true), purge: true });
        assert_eq!(parse_args(&args(&["/S"])).silent, true);
        assert_eq!(parse_args(&args(&["--startup", "--no-startup"])).startup, Some(false), "the last one wins");
        assert_eq!(parse_args(&args(&["--dir"])).dir, None, "a missing value is ignored, not a crash");
        assert_eq!(parse_args(&args(&["--frobnicate"])), d, "unknown flags are ignored");
    }

    #[test]
    fn paths_compare_without_prefixes_case_or_slash_style() {
        assert_eq!(norm(Path::new(r"\\?\C:\Users\Me\AppData\Local\reDraw\redraw.exe")), norm(Path::new("c:/users/me/appdata/local/redraw/REDRAW.EXE")));
        assert_ne!(norm(Path::new(r"C:\a\redraw.exe")), norm(Path::new(r"C:\b\redraw.exe")));
    }

    #[test]
    fn powershell_text_cannot_break_out_of_its_quotes() {
        assert_eq!(ps_quote("O'Brien"), "O''Brien");
        assert_eq!(ps_quote("plain"), "plain");
    }

    #[test]
    fn the_embedded_program_is_a_windows_exe() {
        assert!(EXE.len() > 100 * 1024 && &EXE[..2] == b"MZ", "{} bytes", EXE.len());
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn installing_over_an_existing_exe_keeps_the_old_one_aside() {
        let dir = std::env::temp_dir().join(format!("redraw-setup-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let first = put_exe(&dir, b"first").unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        put_exe(&dir, b"second").unwrap();
        assert_eq!(std::fs::read(dir.join("redraw.exe")).unwrap(), b"second");
        assert_eq!(std::fs::read(dir.join("redraw.exe.old")).unwrap(), b"first");
        put_exe(&dir, b"third").unwrap();
        assert_eq!(std::fs::read(dir.join("redraw.exe.old")).unwrap(), b"second", "one backup, not a pile");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uninstall_refuses_a_folder_it_did_not_make() {
        let dir = std::env::temp_dir().join(format!("redraw-setup-refuse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("precious.txt"), b"keep me").unwrap();
        let o = Opts { silent: true, uninstall: true, dir: Some(dir.clone()), shortcut: false, registry: false, launch: false, startup: None, purge: true };
        let err = uninstall(&o).unwrap_err();
        assert!(err.contains("not a reDraw install folder"), "{err}");
        assert!(dir.join("precious.txt").exists(), "nothing was deleted");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
