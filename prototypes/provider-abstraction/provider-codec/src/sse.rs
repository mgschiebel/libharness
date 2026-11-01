//! Minimal SSE line parser: any chunking, comment lines skipped, multi-line `data:` joined,
//! UTF-8 safe (splits on the `\n` byte, so a multi-byte char cut by a chunk edge is never decoded
//! until its line is complete). Lone `\r` line endings are not supported.
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    data: String,
    has_data: bool,
    started: bool,
    pub comments: usize,
    /// Non-SSE lines (e.g. a plain JSON error body served with 200). Kept for the caller.
    pub stray: String,
}
impl SseParser {
    pub fn push(&mut self, bytes: &[u8], events: &mut Vec<String>) {
        self.buf.extend_from_slice(bytes);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            self.line(&line[..line.len() - 1], events);
        }
    }
    /// End of body: a trailing unterminated line and a pending event are flushed leniently.
    pub fn finish(&mut self, events: &mut Vec<String>) {
        if !self.buf.is_empty() {
            let line = core::mem::take(&mut self.buf);
            self.line(&line, events);
        }
        self.dispatch(events);
    }
    fn dispatch(&mut self, events: &mut Vec<String>) {
        if self.has_data {
            events.push(core::mem::take(&mut self.data));
            self.has_data = false;
        }
    }
    fn line(&mut self, raw: &[u8], events: &mut Vec<String>) {
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        let mut text = String::from_utf8_lossy(raw);
        if !self.started {
            self.started = true;
            if let Some(t) = text.strip_prefix('\u{feff}') {
                text = alloc::borrow::Cow::Owned(String::from(t));
            }
        }
        let text = text.as_ref();
        if text.is_empty() {
            return self.dispatch(events);
        }
        if text.starts_with(':') {
            self.comments += 1;
            return;
        }
        let (field, value) = match text.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (text, ""),
        };
        match field {
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            "event" | "id" | "retry" => {}
            _ => self.stray.push_str(text),
        }
    }
}
