//! One HTTPS GET, on the Windows HTTP stack (WinHTTP): no extra code in the exe, and it uses the
//! system's proxy settings and certificate store. Only the updater uses it, and only to talk to
//! github.com. Blocking: call it from a background thread.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Networking::WinHttp::*;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Closes a WinHTTP handle when dropped.
struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

/// Split `https://host[:port]/path` into (secure, host, port, path). Plain `http://` is accepted
/// only in test builds (`local-update`).
fn split(url: &str) -> Result<(bool, String, u16, String), String> {
    let (secure, rest) = match url.strip_prefix("https://") {
        Some(r) => (true, r),
        None if cfg!(feature = "local-update") => (false, url.strip_prefix("http://").ok_or("not an http address")?),
        None => return Err("only https addresses are used".to_string()),
    };
    let (hostport, path) = rest.split_once('/').map_or((rest, "/".to_string()), |(h, p)| (h, format!("/{p}")));
    let (host, port) = match hostport.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|_| "bad port")?),
        None => (hostport, if secure { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err("no host".to_string());
    }
    Ok((secure, host.to_string(), port, path))
}

/// Fetch `url` (following redirects), reading at most `max` bytes. Anything but HTTP 200 is an error.
pub fn get(url: &str, max: usize) -> Result<Vec<u8>, String> {
    let (secure, host, port, path) = split(url)?;
    unsafe {
        let agent = wide(&format!("reDraw/{}", crate::update::VERSION));
        let mut session = Handle(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0));
        if session.0.is_null() {
            session = Handle(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, null(), null(), 0));
        }
        if session.0.is_null() {
            return Err("the Windows HTTP stack is not available".to_string());
        }
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 30_000);
        let host_w = wide(&host);
        let conn = Handle(WinHttpConnect(session.0, host_w.as_ptr(), port, 0));
        if conn.0.is_null() {
            return Err("cannot connect".to_string());
        }
        let (verb, path_w) = (wide("GET"), wide(&path));
        let req = Handle(WinHttpOpenRequest(conn.0, verb.as_ptr(), path_w.as_ptr(), null(), null(), null(), if secure { WINHTTP_FLAG_SECURE } else { 0 }));
        if req.0.is_null() {
            return Err("cannot open the request".to_string());
        }
        if WinHttpSendRequest(req.0, null(), 0, null(), 0, 0, 0) == 0 || WinHttpReceiveResponse(req.0, null_mut()) == 0 {
            return Err("no response".to_string());
        }
        let mut status: u32 = 0;
        let mut len = std::mem::size_of::<u32>() as u32;
        if WinHttpQueryHeaders(req.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, null(), &mut status as *mut u32 as *mut c_void, &mut len, null_mut()) == 0 {
            return Err("no status".to_string());
        }
        if status != 200 {
            return Err(format!("HTTP {status}"));
        }
        let mut out: Vec<u8> = Vec::new();
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            let mut got: u32 = 0;
            if WinHttpReadData(req.0, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut got) == 0 {
                return Err("the download was interrupted".to_string());
            }
            if got == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buf[..got as usize]);
            if out.len() > max {
                return Err("larger than expected".to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_split_into_host_port_and_path() {
        assert_eq!(split("https://github.com/a/b/releases/latest/download/latest.txt"), Ok((true, "github.com".to_string(), 443, "/a/b/releases/latest/download/latest.txt".to_string())));
        assert_eq!(split("https://example.com"), Ok((true, "example.com".to_string(), 443, "/".to_string())));
        assert_eq!(split("https://example.com:8443/x"), Ok((true, "example.com".to_string(), 8443, "/x".to_string())));
        assert!(split("https://:443/x").is_err() && split("https://h:99999/x").is_err() && split("ftp://h/x").is_err());
        assert_eq!(split("http://h/x").is_ok(), cfg!(feature = "local-update"), "plain http only in test builds");
    }
}
