//! Stop terminal writes after hangup, including Ratatui's cursor-restoring destructor.
use std::{
    io::{self, Write},
    sync::atomic::{AtomicBool, Ordering},
};

static CLOSED: AtomicBool = AtomicBool::new(false);

pub(super) fn closed() -> bool {
    CLOSED.load(Ordering::Relaxed)
}
pub(super) fn reset() {
    CLOSED.store(false, Ordering::Relaxed);
}

pub(crate) struct Output;
impl Output {
    fn finish<T>(result: io::Result<T>, fallback: T) -> io::Result<T> {
        match result {
            // EIO is 5 on both supported Unix platforms; EPIPE has its own ErrorKind.
            Err(error)
                if error.raw_os_error() == Some(5) || error.kind() == io::ErrorKind::BrokenPipe =>
            {
                CLOSED.store(true, Ordering::Relaxed);
                Ok(fallback)
            }
            result => result,
        }
    }
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if closed() {
            return Ok(bytes.len());
        }
        Self::finish(io::stdout().write(bytes), bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        if closed() {
            return Ok(());
        }
        Self::finish(io::stdout().flush(), ())
    }
}
