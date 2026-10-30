//! Stamps the build with its UTC date and time. The menu title shows it, converted to the local time
//! of whoever runs the program, as the "latest update" (so a build made on a server in another time
//! zone still reads right). There is deliberately no `rerun-if-changed`: Cargo then reruns this
//! whenever any file in the package changes, so the stamp is the time of the last edit that was built.

#[cfg(windows)]
fn stamp() -> String {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetSystemTime;
    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetSystemTime(&mut t) };
    format!("{:04}-{:02}-{:02} {:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute)
}

#[cfg(not(windows))]
fn stamp() -> String {
    "0000-00-00 00:00".to_string()
}

fn main() {
    println!("cargo:rustc-env=REDRAW_BUILT_UTC={}", stamp());
    // The release workflow sets REDRAW_VERSION (0.2.<run number>); local builds use Cargo.toml's.
    println!("cargo:rerun-if-env-changed=REDRAW_VERSION");
    let version = std::env::var("REDRAW_VERSION").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_default());
    println!("cargo:rustc-env=REDRAW_VERSION={version}");
}
