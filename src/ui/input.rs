//! Assemble terminal bytes before dispatch; UI work cannot change input timing.
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::{
    io,
    sync::mpsc,
    time::{Duration, Instant},
};

// Allow a little scheduling slack around the terminal's 50 ms escape grace period.
const ESCAPE_GAP: Duration = Duration::from_millis(75);
const PASTE_GAP: Duration = Duration::from_millis(150);
const QUIET: Duration = Duration::from_millis(50);
const START: &[u8] = b"\x1b[200~";
const END: &[u8] = b"\x1b[201~";

pub enum Input {
    Event(Event, Instant),
    Idle(Instant),
    Paused,
    Resumed,
}
enum Command {
    Pause,
    Resume,
    Stop,
}
pub struct Reader {
    commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<io::Result<Input>>,
}
impl Reader {
    pub fn start() -> Self {
        let (commands, requests) = mpsc::channel();
        let (events, input) = mpsc::channel();
        std::thread::spawn(move || {
            let mut paused = false;
            let mut decoder = Decoder::default();
            let mut size = crossterm::terminal::size().ok();
            loop {
                let command = if paused {
                    requests.recv().ok()
                } else {
                    requests.try_recv().ok()
                };
                let result = match command {
                    Some(Command::Stop) => break,
                    Some(Command::Pause | Command::Resume) => {
                        paused = matches!(command, Some(Command::Pause));
                        decoder = Decoder::default();
                        drain();
                        Ok(if paused {
                            Input::Paused
                        } else {
                            Input::Resumed
                        })
                    }
                    None if paused => break,
                    None => {
                        let mut bytes = [0; 4096];
                        let result = read_bytes(&mut bytes, Duration::from_millis(10));
                        let now = Instant::now();
                        match result {
                            Ok(count) => {
                                for event in decoder.feed(&bytes[..count], now) {
                                    // Escape was already held by the decoder; preserve its byte time
                                    // so the UI's defensive terminator grace does not delay it again.
                                    let at = if event == key(KeyCode::Esc, KeyModifiers::NONE) {
                                        decoder.last_byte.unwrap_or(now)
                                    } else {
                                        now
                                    };
                                    if events.send(Ok(Input::Event(event, at))).is_err() {
                                        return;
                                    }
                                }
                                let resized = crossterm::terminal::size().ok();
                                if resized != size {
                                    size = resized;
                                    if let Some((w, h)) = size
                                        && events
                                            .send(Ok(Input::Event(Event::Resize(w, h), now)))
                                            .is_err()
                                    {
                                        return;
                                    }
                                }
                                if decoder.idle(now) {
                                    Ok(Input::Idle(now))
                                } else {
                                    continue;
                                }
                            }
                            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                            Err(error) => Err(error),
                        }
                    }
                };
                let failed = result.is_err();
                if events.send(result).is_err() || failed {
                    break;
                }
            }
        });
        Self {
            commands,
            events: input,
        }
    }
    pub fn pause(&self) -> io::Result<()> {
        self.command(Command::Pause, true)
    }
    pub fn resume(&self) -> io::Result<()> {
        self.command(Command::Resume, false)
    }
    fn command(&self, command: Command, paused: bool) -> io::Result<()> {
        self.commands
            .send(command)
            .map_err(|_| io::Error::other("terminal reader stopped"))?;
        loop {
            // The reader only performs bounded raw reads, never a blocking paste decode.
            let input = self
                .events
                .recv()
                .map_err(|_| io::Error::other("terminal reader stopped"))??;
            if matches!(
                (paused, input),
                (true, Input::Paused) | (false, Input::Resumed)
            ) {
                return Ok(());
            }
        }
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
    }
}

// stdin is verified to be a TTY. Raw reads avoid buffered bytes during child handoff.
#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}
#[cfg(target_os = "linux")]
type PollCount = std::os::raw::c_ulong;
#[cfg(not(target_os = "linux"))]
type PollCount = u32;
unsafe extern "C" {
    fn poll(fds: *mut PollFd, count: PollCount, timeout: i32) -> i32;
    fn read(fd: i32, bytes: *mut std::ffi::c_void, count: usize) -> isize;
}
fn read_bytes(bytes: &mut [u8], timeout: Duration) -> io::Result<usize> {
    let mut fd = PollFd {
        fd: 0,
        events: 1,
        revents: 0,
    };
    // SAFETY: one valid pollfd is writable for the duration of the call.
    let ready = unsafe { poll(&mut fd, 1, timeout.as_millis() as i32) };
    if ready < 0 {
        return Err(io::Error::last_os_error());
    }
    if ready == 0 {
        return Ok(0);
    }
    // SAFETY: bytes is writable for count bytes; this thread is the only board reader.
    let count = unsafe { read(0, bytes.as_mut_ptr().cast(), bytes.len()) };
    if count < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(count as usize)
    }
}
fn drain() {
    let deadline = Instant::now() + Duration::from_millis(100);
    let mut quiet_since = Instant::now();
    let mut bytes = [0; 4096];
    while Instant::now() < deadline {
        match read_bytes(&mut bytes, Duration::from_millis(10)) {
            Ok(0) if quiet_since.elapsed() >= ESCAPE_GAP => break,
            Ok(0) => {}
            Err(_) => break,
            Ok(_) => quiet_since = Instant::now(),
        }
    }
}

#[derive(Default)]
struct Decoder {
    pending: Vec<u8>,
    paste: Option<Vec<u8>>,
    last_byte: Option<Instant>,
    last_idle: Option<Instant>,
}
impl Decoder {
    fn idle(&mut self, now: Instant) -> bool {
        if !self.pending.is_empty() || self.paste.is_some() {
            return false;
        }
        if self
            .last_idle
            .is_none_or(|at| now.duration_since(at) >= QUIET)
            && self
                .last_byte
                .is_none_or(|at| now.duration_since(at) >= QUIET)
        {
            self.last_idle = Some(now);
            return true;
        }
        false
    }
    fn feed(&mut self, bytes: &[u8], now: Instant) -> Vec<Event> {
        let mut events = Vec::new();
        if bytes.is_empty()
            && let Some(last) = self.last_byte
        {
            let gap = now.duration_since(last);
            if self.paste.is_some() && gap >= PASTE_GAP {
                self.paste = None;
                self.pending.clear();
            } else if self.paste.is_none() && !self.pending.is_empty() && gap >= ESCAPE_GAP {
                if self.pending == [27] || self.pending == [27, 27] {
                    events.push(key(KeyCode::Esc, KeyModifiers::NONE));
                }
                self.pending.clear();
            }
        }
        if !bytes.is_empty() {
            self.last_byte = Some(now);
        }
        self.pending.extend_from_slice(bytes);
        loop {
            if self.pending.is_empty() {
                break;
            }
            if let Some(paste) = &mut self.paste {
                if self.pending.starts_with(END) {
                    self.pending.drain(..END.len());
                    events.push(Event::Paste(String::from_utf8_lossy(paste).into_owned()));
                    self.paste = None;
                } else if self.pending.starts_with(START) {
                    self.pending.drain(..START.len());
                } else if START.starts_with(&self.pending) || END.starts_with(&self.pending) {
                    break;
                } else {
                    // Prompts are capped separately; bound an unterminated paste too.
                    if paste.len() < 1024 * 1024 {
                        paste.push(self.pending[0]);
                    }
                    self.pending.remove(0);
                }
                continue;
            }
            if self.pending.starts_with(START) {
                self.pending.drain(..START.len());
                self.paste = Some(Vec::new());
                continue;
            }
            if self.pending.starts_with(END) {
                self.pending.drain(..END.len());
                continue;
            }
            let mut offset = 0;
            let mut modifiers = KeyModifiers::NONE;
            if self.pending[0] == 27 {
                if self.pending.len() == 1 {
                    break;
                }
                if self.pending[1] == 27 {
                    // Wait to distinguish a bare pair from Escape followed by a sequence.
                    if self.pending.len() == 2 {
                        break;
                    }
                    events.push(key(KeyCode::Esc, KeyModifiers::NONE));
                    self.pending.drain(..1);
                    continue;
                }
                if matches!(self.pending[1], b'[' | b'O') {
                    // Linux console F1-F5 have an extra [ before the final byte.
                    let start = if self.pending.starts_with(b"\x1b[[") {
                        3
                    } else {
                        2
                    };
                    let Some(end) = self.pending[start..]
                        .iter()
                        .position(|b| *b < 32 || (0x40..=0x7e).contains(b))
                    else {
                        break;
                    };
                    let end = end + start;
                    if self.pending[end] < 32 {
                        self.pending.drain(..end);
                        continue;
                    }
                    if start == 2
                        && let Some(code) = sequence(&self.pending[start..=end])
                    {
                        events.push(key(code, KeyModifiers::NONE));
                    }
                    self.pending.drain(..=end);
                    continue;
                }
                offset = 1;
                modifiers = KeyModifiers::ALT;
            }
            let byte = self.pending[offset];
            let (code, count) = match byte {
                b'\r' | b'\n' => (KeyCode::Enter, 1),
                b'\t' => (KeyCode::Tab, 1),
                127 | 8 => (KeyCode::Backspace, 1),
                1..=26 => {
                    modifiers |= KeyModifiers::CONTROL;
                    (KeyCode::Char((b'a' + byte - 1) as char), 1)
                }
                0 => (KeyCode::Null, 1),
                _ => {
                    let width = if byte < 128 {
                        1
                    } else {
                        byte.leading_ones() as usize
                    };
                    if !(1..=4).contains(&width) {
                        self.pending.drain(..=offset);
                        continue;
                    }
                    if let Some(control) = self.pending
                        [offset + 1..self.pending.len().min(offset + width)]
                        .iter()
                        .position(|b| *b < 32)
                    {
                        self.pending.drain(..offset + 1 + control);
                        continue;
                    }
                    if self.pending.len() < offset + width {
                        break;
                    }
                    let text = std::str::from_utf8(&self.pending[offset..offset + width]);
                    let Ok(text) = text else {
                        self.pending.drain(..=offset);
                        continue;
                    };
                    (
                        KeyCode::Char(
                            text.chars()
                                .next()
                                .expect("validated UTF-8 slice is nonempty"),
                        ),
                        width,
                    )
                }
            };
            events.push(key(code, modifiers));
            self.pending.drain(..offset + count);
        }
        events
    }
}
fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, modifiers))
}
fn sequence(bytes: &[u8]) -> Option<KeyCode> {
    Some(match bytes {
        b"A" => KeyCode::Up,
        b"B" => KeyCode::Down,
        b"C" => KeyCode::Right,
        b"D" => KeyCode::Left,
        b"H" | b"1~" | b"7~" => KeyCode::Home,
        b"F" | b"4~" | b"8~" => KeyCode::End,
        b"2~" => KeyCode::Insert,
        b"3~" => KeyCode::Delete,
        b"5~" => KeyCode::PageUp,
        b"6~" => KeyCode::PageDown,
        b"Z" => KeyCode::BackTab,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_escape_and_every_paste_delimiter_byte_join_within_grace() {
        for gap in [1, 20, 40, 50] {
            let mut decoder = Decoder::default();
            let mut now = Instant::now();
            let mut events = Vec::new();
            for byte in b"\x1b[200~bar\x1b[201~" {
                events.extend(decoder.feed(&[*byte], now));
                now += Duration::from_millis(gap);
            }
            assert_eq!(events, [Event::Paste("bar".into())]);
        }
        let mut decoder = Decoder::default();
        let now = Instant::now();
        assert!(decoder.feed(b"\x1b", now).is_empty());
        assert_eq!(
            decoder.feed(b"p", now + QUIET),
            [key(KeyCode::Char('p'), KeyModifiers::ALT)]
        );
    }

    #[test]
    fn lone_escape_expires_and_incomplete_sequences_do_not_dispatch_their_tail() {
        let mut decoder = Decoder::default();
        let now = Instant::now();
        assert!(decoder.feed(b"\x1b", now).is_empty());
        assert!(decoder.feed(b"", now + QUIET).is_empty());
        assert_eq!(
            decoder.feed(b"", now + ESCAPE_GAP),
            [key(KeyCode::Esc, KeyModifiers::NONE)]
        );
        assert!(
            decoder
                .feed(b"\x1b[20", now + Duration::from_secs(1))
                .is_empty()
        );
        assert!(decoder.feed(b"", now + Duration::from_secs(2)).is_empty());
    }

    #[test]
    fn paste_timeout_recovers_control_keys_and_nested_delimiters_are_removed() {
        let mut decoder = Decoder::default();
        let now = Instant::now();
        assert!(decoder.feed(b"\x1b[200~foo", now).is_empty());
        assert!(!decoder.idle(now + QUIET));
        assert!(decoder.feed(b"", now + PASTE_GAP).is_empty());
        assert_eq!(
            decoder.feed(b"\x03q", now + PASTE_GAP),
            [
                key(KeyCode::Char('c'), KeyModifiers::CONTROL),
                key(KeyCode::Char('q'), KeyModifiers::NONE),
            ]
        );
        assert_eq!(
            decoder.feed(
                b"\x1b[200~foo\x1b[200~bar\x1b[201~\x1b[201~",
                now + Duration::from_secs(1)
            ),
            [Event::Paste("foobar".into())]
        );
    }

    #[test]
    fn navigation_and_split_unicode_survive_raw_input() {
        let mut decoder = Decoder::default();
        let now = Instant::now();
        assert_eq!(
            decoder.feed(b"\x1b[A\x1bOB\x1b[5~", now),
            [
                key(KeyCode::Up, KeyModifiers::NONE),
                key(KeyCode::Down, KeyModifiers::NONE),
                key(KeyCode::PageUp, KeyModifiers::NONE),
            ]
        );
        assert!(decoder.feed(&[0xc3], now).is_empty());
        assert_eq!(
            decoder.feed(&[0xa9], now),
            [key(KeyCode::Char('é'), KeyModifiers::NONE)]
        );
    }

    #[test]
    fn issue111_linux_console_function_keys_never_dispatch_a_suffix() {
        for final_byte in b'A'..=b'E' {
            let bytes = [27, b'[', b'[', final_byte];
            for split in 0..=bytes.len() {
                let mut decoder = Decoder::default();
                let now = Instant::now();
                assert!(decoder.feed(&bytes[..split], now).is_empty());
                assert!(
                    decoder
                        .feed(&bytes[split..], now + Duration::from_millis(20))
                        .is_empty()
                );
                assert!(decoder.pending.is_empty());
            }
        }
    }

    #[test]
    fn issue111_double_escape_is_an_unmodified_escape() {
        for split in 0..=2 {
            let mut decoder = Decoder::default();
            let now = Instant::now();
            let mut events = decoder.feed(&b"\x1b\x1b"[..split], now);
            events.extend(decoder.feed(&b"\x1b\x1b"[split..], now + Duration::from_millis(20)));
            assert!(events.is_empty());
            events.extend(decoder.feed(b"", now + Duration::from_millis(20) + ESCAPE_GAP));
            assert_eq!(events, [key(KeyCode::Esc, KeyModifiers::NONE)]);
        }
    }

    #[test]
    fn issue111_controls_abort_partial_csi_and_utf8() {
        for prefix in [
            b"\x1b[".as_slice(),
            b"\x1b[20",
            b"\xf0",
            b"\xf0\x9f",
            b"\xf0\x9f\x92",
        ] {
            for control in 0..32 {
                let mut decoder = Decoder::default();
                let now = Instant::now();
                assert!(decoder.feed(prefix, now).is_empty());
                let mut events = decoder.feed(&[control], now + Duration::from_millis(20));
                events.extend(decoder.feed(b"", now + Duration::from_millis(20) + ESCAPE_GAP));
                let mut clean = Decoder::default();
                let mut expected = clean.feed(&[control], now);
                expected.extend(clean.feed(b"", now + ESCAPE_GAP));
                assert_eq!(events, expected, "prefix={prefix:?}, control={control}");
                assert!(!events.is_empty());
            }
        }
    }

    #[test]
    fn issue111_paste_buffer_is_capped_at_one_mib() {
        let mut decoder = Decoder::default();
        let now = Instant::now();
        assert!(decoder.feed(START, now).is_empty());
        let chunk = [b'a'; 4096];
        for _ in 0..256 {
            assert!(decoder.feed(&chunk, now).is_empty());
        }
        assert!(decoder.feed(b"overflow", now).is_empty());
        assert_eq!(decoder.paste.as_ref().unwrap().len(), 1024 * 1024);
        let events = decoder.feed(END, now);
        let [Event::Paste(text)] = events.as_slice() else {
            panic!("expected one paste")
        };
        assert_eq!(text.len(), 1024 * 1024);
        assert!(text.bytes().all(|b| b == b'a'));
        assert!(decoder.paste.is_none());
    }

    #[test]
    fn issue111_unsupported_sequences_are_ignored_as_complete_sequences() {
        for bytes in [
            b"\x1b[13u".as_slice(),
            b"\x1b[99;5u",
            b"\x1b[1;5B",
            b"\x1bOP",
            b"\x1b[15~",
            b"\x1b[I",
            b"\x1b[O",
            b"\x1b[?1;2c",
            b"\x1b[30;100R",
        ] {
            for split in 0..=bytes.len() {
                let mut decoder = Decoder::default();
                let now = Instant::now();
                assert!(decoder.feed(&bytes[..split], now).is_empty());
                assert!(
                    decoder
                        .feed(&bytes[split..], now + Duration::from_millis(20))
                        .is_empty()
                );
                assert!(decoder.pending.is_empty());
                assert_eq!(
                    decoder.feed(b"x", now + Duration::from_millis(40)),
                    [key(KeyCode::Char('x'), KeyModifiers::NONE)]
                );
            }
        }
    }

    #[test]
    fn issue114_escape_before_a_sequence_keeps_the_second_introducer() {
        let esc = key(KeyCode::Esc, KeyModifiers::NONE);
        for (bytes, expected) in [
            (
                b"\x1b\x1b[A".as_slice(),
                vec![esc.clone(), key(KeyCode::Up, KeyModifiers::NONE)],
            ),
            (
                b"\x1b\x1bOA",
                vec![esc.clone(), key(KeyCode::Up, KeyModifiers::NONE)],
            ),
            (
                b"\x1b\x1b[200~a\x1b[201~",
                vec![esc.clone(), Event::Paste("a".into())],
            ),
            (b"\x1b\x1b", vec![esc.clone()]),
            (b"\x1b\x1b\x1b", vec![esc.clone(), esc]),
        ] {
            for split in 0..=bytes.len() {
                let mut decoder = Decoder::default();
                let now = Instant::now();
                let mut events = decoder.feed(&bytes[..split], now);
                events.extend(decoder.feed(&bytes[split..], now + Duration::from_millis(20)));
                events.extend(decoder.feed(b"", now + Duration::from_millis(20) + ESCAPE_GAP));
                assert_eq!(events, expected, "bytes={bytes:?}, split={split}");
            }
            let mut decoder = Decoder::default();
            let mut now = Instant::now();
            let mut events = Vec::new();
            for byte in bytes {
                events.extend(decoder.feed(&[*byte], now));
                now += Duration::from_millis(20);
            }
            events.extend(decoder.feed(b"", now + ESCAPE_GAP));
            assert_eq!(events, expected, "bytewise={bytes:?}");
        }
    }
}
