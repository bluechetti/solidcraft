//! Linux and other Unix systems (not macOS): who ended the app with a signal (docs/crash-reports.md).
//!
//! An app that is stopped by SIGTERM, SIGHUP, SIGINT or SIGQUIT (a terminal or session that closed
//! under it, a script's `kill`, a service manager) leaves no panic, no core file and no log, and
//! the next start only found "an unclean exit". A handler for those four signals now writes,
//! before the process goes, the same small JSON file as the native crash handlers: the signal and
//! the process that sent it (its id and its short name from `/proc/<pid>/comm`) next to the run's
//! crash journal (`solidcraft_engine::crash::native_paths`). The next start puts it into the unclean
//! exit's report. The process then dies of the signal exactly as it would have (the default action
//! is put back and the signal sent again), so nothing about how the app ends changes.
//!
//! SIGKILL cannot be caught: an app killed that way (the kernel's OOM killer, `systemd-oomd`)
//! still reports an unclean exit with no signal, which is itself the hint.
//!
//! Installing a signal handler has no safe API, hence the `unsafe_code` allowance on this module
//! (the app crate denies it). The handler only calls what POSIX lists as async-signal-safe (open,
//! read, write, close, sigaction, raise) and formats into a buffer on the stack. A signal that
//! already has a handler (anyone else's) is left alone.
#![allow(unsafe_code)]

use std::ffi::{CString, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use libc::{SIGHUP, SIGINT, SIGQUIT, SIGTERM, siginfo_t};

/// The signals that end the app from outside.
const SIGNALS: [(c_int, &str); 4] = [(SIGTERM, "SIGTERM"), (SIGHUP, "SIGHUP"), (SIGINT, "SIGINT"), (SIGQUIT, "SIGQUIT")];

/// Where the handler writes (NUL-terminated), set once by [`install`].
static PATH: OnceLock<CString> = OnceLock::new();
/// The handler has started writing (a second signal while it does is only passed on).
static BUSY: AtomicBool = AtomicBool::new(false);

/// Record signals that end the app from now on (after `solidcraft_engine::crash::install`; nothing
/// without it).
pub fn install() {
    let Some((info, _)) = solidcraft_engine::crash::native_paths() else { return };
    let Ok(path) = CString::new(info.as_os_str().as_bytes()) else { return };
    if PATH.set(path).is_err() {
        return;
    }
    for (signal, _) in SIGNALS {
        // SAFETY: `sigaction` is plain data; all zeros is a valid (empty) value.
        let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: a null new action only reads the current one into `old`.
        if unsafe { libc::sigaction(signal, std::ptr::null(), &mut old) } != 0 || old.sa_sigaction != libc::SIG_DFL {
            continue;
        }
        // SAFETY: as above.
        let mut ours: libc::sigaction = unsafe { std::mem::zeroed() };
        ours.sa_sigaction = handler as *const () as usize;
        ours.sa_flags = libc::SA_SIGINFO | libc::SA_RESETHAND;
        // SAFETY: `ours` is a valid action and the mask is initialised by `sigemptyset`.
        unsafe {
            libc::sigemptyset(&mut ours.sa_mask);
            libc::sigaction(signal, &ours, std::ptr::null_mut());
        }
    }
}

/// A fixed-size text buffer on the stack.
struct Buf {
    bytes: [u8; 512],
    len: usize,
}

impl Buf {
    fn text(&mut self, s: &str) {
        self.plain(s.as_bytes());
    }

    fn plain(&mut self, s: &[u8]) {
        for &b in s {
            // A quote or control character would break the JSON: keep the text plain.
            let b = if b == b'"' || b == b'\\' || b < b' ' { b'_' } else { b };
            if let Some(slot) = self.bytes.get_mut(self.len) {
                *slot = b;
                self.len += 1;
            }
        }
    }

    fn raw(&mut self, b: &[u8]) {
        for &b in b {
            if let Some(slot) = self.bytes.get_mut(self.len) {
                *slot = b;
                self.len += 1;
            }
        }
    }

    fn number(&mut self, mut n: u64) {
        let mut digits = [0u8; 20];
        let mut at = digits.len();
        loop {
            at -= 1;
            if let Some(d) = digits.get_mut(at) {
                *d = b'0' + (n % 10) as u8;
            }
            n /= 10;
            if n == 0 || at == 0 {
                break;
            }
        }
        if let Some(rest) = digits.get(at..) {
            self.raw(rest);
        }
    }
}

/// The signal handler: record, then die of the signal as the default action would.
unsafe extern "C" fn handler(signal: c_int, info: *mut siginfo_t, _context: *mut c_void) {
    if !BUSY.swap(true, Ordering::SeqCst) {
        // SAFETY: the system passes a valid siginfo for the duration of an SA_SIGINFO handler.
        let sender = if info.is_null() { 0 } else { unsafe { (*info).si_pid() } };
        record(signal, sender);
    }
    // SA_RESETHAND put the default action back: send the signal again.
    // SAFETY: `raise` is async-signal-safe.
    unsafe { libc::raise(signal) };
}

fn record(signal: c_int, sender: libc::pid_t) {
    let Some(path) = PATH.get() else { return };
    let name = SIGNALS.iter().find(|(s, _)| *s == signal).map_or("signal", |(_, n)| n);
    let mut out = Buf { bytes: [0; 512], len: 0 };
    out.raw(b"{\"code\":\"");
    out.text(name);
    out.raw(b"\",\"name\":\"signal\",\"sent_by\":");
    out.number(u64::try_from(sender).unwrap_or(0));
    out.raw(b",\"sender\":\"");
    if sender > 0 {
        // "/proc/<pid>/comm"
        let mut file = Buf { bytes: [0; 512], len: 0 };
        file.raw(b"/proc/");
        file.number(u64::try_from(sender).unwrap_or(0));
        file.raw(b"/comm");
        file.raw(&[0]);
        // SAFETY: the buffer is NUL-terminated just above.
        let fd = unsafe { libc::open(file.bytes.as_ptr().cast(), libc::O_RDONLY | libc::O_CLOEXEC) };
        if fd >= 0 {
            let mut comm = [0u8; 64];
            // SAFETY: `comm` is a valid buffer of the length given.
            let n = unsafe { libc::read(fd, comm.as_mut_ptr().cast(), comm.len()) };
            // SAFETY: `fd` was opened above.
            unsafe { libc::close(fd) };
            if let Some(read) = usize::try_from(n).ok().and_then(|n| comm.get(..n)) {
                let text = read.strip_suffix(b"\n").unwrap_or(read);
                out.plain(text);
            }
        }
    }
    out.raw(b"\",\"module\":\"\",\"offset\":\"\",\"address\":\"\"}");
    // SAFETY: `path` is NUL-terminated; the buffer holds `len` bytes.
    unsafe {
        let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC, 0o644);
        if fd >= 0 {
            libc::write(fd, out.bytes.as_ptr().cast(), out.len);
            libc::close(fd);
        }
    }
}
