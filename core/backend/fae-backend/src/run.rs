//! Running the external encoders (cjpegli, SvtAv1EncApp) (B4; proc.py): no console window, and for the
//! encoded compare at below-normal priority and cancellable: a newer request kills the older one's process.
//! Process is cancellable too: `stop_process` kills its encoder run and ends the batch.

use crate::{Error, ErrorKind};
use std::ffi::OsString;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// What an encoder run gave: exit code (None: killed by a signal) and its output.
pub struct Output {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// The encoder's message for an error: stderr, else stdout, the last `n` characters.
    pub fn message(&self, n: usize) -> String {
        let m = if self.stderr.is_empty() { &self.stdout } else { &self.stderr };
        let s = String::from_utf8_lossy(m);
        let s = s.trim();
        let skip = s.chars().count().saturating_sub(n);
        s.chars().skip(skip).collect()
    }

    pub fn code_str(&self) -> String {
        self.code.map_or("killed".into(), |c| c.to_string())
    }
}

/// The encoded compare's requests: `latest` is the newest request id; runs of older ones stop.
/// `stopped`: Process was cancelled (process_cancel); its runs stop until the next Process begins.
#[derive(Default)]
pub struct Runner {
    latest: Mutex<i64>,
    stopped: AtomicBool,
}

pub fn cancelled() -> Error {
    Error::new(ErrorKind::Cancelled, "cancelled")
}

impl Runner {
    pub fn new() -> Runner {
        Runner { latest: Mutex::new(-1), stopped: AtomicBool::new(false) }
    }

    /// A Process batch begins: an earlier cancel no longer applies.
    pub fn begin_process(&self) {
        self.stopped.store(false, Ordering::SeqCst);
    }

    /// Cancel the running Process batch (process_cancel): its encoder run is killed, the batch ends
    /// after the image it's on.
    pub fn stop_process(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    /// A new encoded-compare request (proc.py `preview_begin`): runs of older requests are killed.
    pub fn begin(&self, req: i64) {
        let mut l = self.latest.lock().unwrap();
        if req > *l {
            *l = req;
        }
    }

    pub fn latest(&self) -> i64 {
        *self.latest.lock().unwrap()
    }

    /// Err(Cancelled) when `req` (an encoded-compare request) was replaced, or (None: Process) when
    /// Process was cancelled.
    pub fn check(&self, req: Option<i64>) -> Result<(), Error> {
        match req {
            Some(r) if r < self.latest() => Err(cancelled()),
            None if self.stopped.load(Ordering::SeqCst) => Err(cancelled()),
            _ => Ok(()),
        }
    }

    /// Run `cmd` to its end (proc.py `run_encoder`). With `req`: below-normal priority, killed as soon as
    /// a newer request begins (Err(Cancelled)); without: killed when Process is cancelled.
    pub fn run(&self, cmd: &[OsString], req: Option<i64>) -> Result<Output, Error> {
        self.check(req)?;
        let mut c = Command::new(&cmd[0]);
        c.args(&cmd[1..]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW (pythonw has no console: every call would flash one), + BELOW_NORMAL_PRIORITY_CLASS
            c.creation_flags(if req.is_some() { 0x0800_0000 | 0x0000_4000 } else { 0x0800_0000 });
        }
        let mut child = c
            .spawn()
            .map_err(|e| Error::new(ErrorKind::Failed, format!("{}: {e}", std::path::Path::new(&cmd[0]).display())))?;
        let drain = |r: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut v = vec![];
                if let Some(mut r) = r {
                    let _ = r.read_to_end(&mut v);
                }
                v
            })
        };
        let out = drain(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
        let err = drain(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) => {}
                Err(_) => break None,
            }
            if self.check(req).is_err() {
                let _ = child.kill();
                let _ = child.wait();
                // the readers end on their own (a grandchild may still hold the pipes)
                return Err(cancelled());
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let status = match status {
            Some(s) => s,
            None => child.wait().map_err(|e| Error::new(ErrorKind::Failed, format!("encoder: {e}")))?,
        };
        let o = Output { code: status.code(), stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default() };
        self.check(req)?;
        Ok(o)
    }
}

/// A folder for an encoder's input / output files, removed with everything in it when dropped.
pub struct TempDir(pub std::path::PathBuf);

impl TempDir {
    pub fn new(prefix: &str) -> Result<TempDir, Error> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        for _ in 0..100 {
            let n = N.fetch_add(1, Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("{prefix}{}_{t:x}_{n}", std::process::id()));
            match std::fs::create_dir(&p) {
                Ok(()) => return Ok(TempDir(p)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(Error::new(ErrorKind::Io, format!("temp folder: {e}"))),
            }
        }
        Err(Error::new(ErrorKind::Io, "temp folder: no free name"))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn runs_and_cancels() {
        let r = Arc::new(Runner::new());
        let o = r.run(&["sh".into(), "-c".into(), "echo hi; echo oops >&2; exit 3".into()], None).unwrap();
        assert_eq!((o.code, o.stdout.as_slice(), o.message(100).as_str()), (Some(3), &b"hi\n"[..], "oops"));
        r.begin(5);
        let r2 = r.clone();
        let t = std::thread::spawn(move || r2.run(&["sleep".into(), "5".into()], Some(5)));
        std::thread::sleep(Duration::from_millis(100));
        let t0 = std::time::Instant::now();
        r.begin(6);
        assert_eq!(t.join().unwrap().err().unwrap().kind, ErrorKind::Cancelled);
        assert!(t0.elapsed() < Duration::from_secs(2));
        assert_eq!(r.run(&["true".into()], Some(5)).err().unwrap().kind, ErrorKind::Cancelled);
        // Process (no request): stop_process kills its run; the next batch runs again
        let r2 = r.clone();
        let t = std::thread::spawn(move || r2.run(&["sleep".into(), "5".into()], None));
        std::thread::sleep(Duration::from_millis(100));
        let t0 = std::time::Instant::now();
        r.stop_process();
        assert_eq!(t.join().unwrap().err().unwrap().kind, ErrorKind::Cancelled);
        assert!(t0.elapsed() < Duration::from_secs(2));
        assert!(r.run(&["true".into()], Some(7)).is_ok()); // the encoded compare isn't stopped by it
        r.begin_process();
        assert!(r.run(&["true".into()], None).is_ok());
        let d = TempDir::new("fae_test_").unwrap();
        let p = d.0.clone();
        std::fs::write(p.join("x"), b"1").unwrap();
        drop(d);
        assert!(!p.exists());
    }
}
