//! Terminal control with ANSI escapes and `stty` only: alternate screen, hidden cursor, raw-ish input
//! (so Ctrl-C arrives as a byte we can clean up on), restore on exit and on panic.
use std::fs::File;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

static SAVED: Mutex<Option<String>> = Mutex::new(None);

fn tty() -> Option<File> {
    File::open("/dev/tty").ok()
}

fn stty(args: &[&str]) -> Option<String> {
    let out = Command::new("stty").args(args).stdin(tty()?).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn restore() {
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x1b[?2026l\x1b[?25h\x1b[?1049l");
    let _ = out.flush();
    if let Some(s) = SAVED.lock().ok().and_then(|mut g| g.take()) {
        stty(&[&s]);
    }
}

pub struct Term;

impl Term {
    pub fn enter() -> Term {
        *SAVED.lock().unwrap() = stty(&["-g"]);
        stty(&["-icanon", "-echo", "-isig"]);
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |i| {
            restore();
            prev(i);
        }));
        print!("\x1b[?1049h\x1b[?25l\x1b[2J");
        let _ = std::io::stdout().flush();
        Term
    }

    /// Flag that turns true when the user presses q, Esc or Ctrl-C.
    pub fn quit_flag(&self) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        let f = flag.clone();
        if let Some(mut t) = tty() {
            std::thread::spawn(move || {
                let mut b = [0u8; 1];
                while t.read(&mut b).is_ok_and(|n| n == 1) {
                    if matches!(b[0], 3 | b'q' | b'Q' | 27) {
                        f.store(true, Relaxed);
                    }
                }
            });
        }
        flag
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        restore();
    }
}

/// (columns, rows). Falls back to $COLUMNS/$LINES, then 100x30.
pub fn size() -> (usize, usize) {
    if let Some(s) = stty(&["size"]) {
        let mut p = s.split_whitespace().map(|x| x.parse::<usize>().unwrap_or(0));
        if let (Some(r), Some(c)) = (p.next(), p.next()) {
            if r > 0 && c > 0 {
                return (c, r);
            }
        }
    }
    let e = |k: &str, d: usize| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    (e("COLUMNS", 100), e("LINES", 30))
}
