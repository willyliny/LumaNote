//! Incremental Server-Sent-Events parser shared by both protocol adapters.
//!
//! Feed it raw byte chunks of any size (a chunk may end in the middle of a UTF-8
//! character or of a line); it returns each event as soon as its terminating blank line
//! arrives. Handles `\n`, `\r\n` and bare `\r` line ends, multi-line `data:`, comment /
//! keep-alive lines (`: ...`), `event:` names, `id:` and a leading byte-order mark.
//!
//! The `[DONE]` sentinel of OpenAI-style streams is an ordinary event whose data is
//! `[DONE]`; see [`SseEvent::is_done`].

use std::fmt;

/// Longest single line accepted (guards memory against a stream without line breaks).
pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
/// Longest event (all `data:` lines together) accepted.
pub const MAX_EVENT_BYTES: usize = 16 * 1024 * 1024;

/// One dispatched event.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` name, if the server sent one.
    pub event: Option<String>,
    /// The `data:` lines joined with `\n`.
    pub data: String,
    /// The `id:` field, if any.
    pub id: Option<String>,
}

impl SseEvent {
    /// True for the OpenAI-style `data: [DONE]` end marker.
    pub fn is_done(&self) -> bool {
        self.data.trim() == "[DONE]"
    }
}

/// The stream broke a limit of the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseError {
    /// A single line exceeded [`MAX_LINE_BYTES`].
    LineTooLong,
    /// One event exceeded [`MAX_EVENT_BYTES`].
    EventTooLarge,
}

impl fmt::Display for SseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SseError::LineTooLong => f.write_str("SSE line too long"),
            SseError::EventTooLarge => f.write_str("SSE event too large"),
        }
    }
}

impl std::error::Error for SseError {}

/// Incremental SSE parser.
#[derive(Debug, Default)]
pub struct SseParser {
    line: Vec<u8>,
    skip_lf: bool,
    seen_any_byte: bool,
    event: Option<String>,
    id: Option<String>,
    data: Option<String>,
    data_bytes: usize,
}

impl SseParser {
    /// Creates a parser.
    pub fn new() -> Self {
        Self::default()
    }

    /// Consumes a chunk and returns the events it completed.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, SseError> {
        let mut out = Vec::new();
        for &b in chunk {
            if self.skip_lf {
                self.skip_lf = false;
                if b == b'\n' {
                    continue;
                }
            }
            match b {
                b'\r' => {
                    self.skip_lf = true;
                    self.end_line(&mut out)?;
                }
                b'\n' => self.end_line(&mut out)?,
                _ => {
                    if self.line.len() >= MAX_LINE_BYTES {
                        return Err(SseError::LineTooLong);
                    }
                    self.line.push(b);
                }
            }
        }
        Ok(out)
    }

    /// Call at end of stream. A complete `data:` line that was never followed by a blank
    /// line is still dispatched (some servers end the stream that way); a line cut off in
    /// the middle is discarded.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        self.line.clear();
        self.skip_lf = false;
        self.dispatch().into_iter().collect()
    }

    fn end_line(&mut self, out: &mut Vec<SseEvent>) -> Result<(), SseError> {
        let mut raw = std::mem::take(&mut self.line);
        if !self.seen_any_byte {
            self.seen_any_byte = true;
            if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
                raw.drain(..3);
            }
        }
        if raw.is_empty() {
            out.extend(self.dispatch());
            return Ok(());
        }
        let line = String::from_utf8_lossy(&raw);
        if line.starts_with(':') {
            return Ok(());
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line.as_ref(), ""),
        };
        match field {
            "event" => self.event = Some(value.to_owned()),
            "id" => {
                if !value.contains('\0') {
                    self.id = Some(value.to_owned())
                }
            }
            "data" => {
                self.data_bytes += value.len() + 1;
                if self.data_bytes > MAX_EVENT_BYTES {
                    return Err(SseError::EventTooLarge);
                }
                match &mut self.data {
                    Some(d) => {
                        d.push('\n');
                        d.push_str(value);
                    }
                    None => self.data = Some(value.to_owned()),
                }
            }
            _ => {} // retry and unknown fields are ignored
        }
        Ok(())
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        let event = self.event.take();
        let id = self.id.take();
        self.data_bytes = 0;
        let data = self.data.take()?;
        Some(SseEvent { event, data, id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(input: &[u8], chunk: usize) -> Vec<SseEvent> {
        let mut p = SseParser::new();
        let mut out = Vec::new();
        for c in input.chunks(chunk.max(1)) {
            out.extend(p.push(c).unwrap());
        }
        out.extend(p.finish());
        out
    }

    #[test]
    fn named_events_and_comments() {
        let input = b": header comment\n\nevent: ping\ndata: {\"type\": \"ping\"}\n\nevent: message_stop\ndata: {}\n\n";
        for size in [1, 2, 3, 7, 1000] {
            let evs = parse_all(input, size);
            assert_eq!(evs.len(), 2, "chunk {size}");
            assert_eq!(evs[0].event.as_deref(), Some("ping"));
            assert_eq!(evs[0].data, "{\"type\": \"ping\"}");
            assert_eq!(evs[1].event.as_deref(), Some("message_stop"));
        }
    }

    #[test]
    fn crlf_cr_and_lf_line_ends() {
        for sep in ["\n", "\r\n", "\r"] {
            let input = format!("data: a{sep}data: b{sep}{sep}data: c{sep}{sep}");
            for size in [1, 2, 5, 100] {
                let evs = parse_all(input.as_bytes(), size);
                assert_eq!(
                    evs.iter().map(|e| e.data.as_str()).collect::<Vec<_>>(),
                    vec!["a\nb", "c"],
                    "sep {sep:?} chunk {size}"
                );
            }
        }
    }

    #[test]
    fn crlf_split_between_chunks_is_one_line_end() {
        let mut p = SseParser::new();
        let mut evs = p.push(b"data: x\r").unwrap();
        evs.extend(p.push(b"\n\r").unwrap());
        evs.extend(p.push(b"\ndata: y\r\n\r\n").unwrap());
        assert_eq!(
            evs.iter().map(|e| e.data.as_str()).collect::<Vec<_>>(),
            vec!["x", "y"]
        );
    }

    #[test]
    fn multibyte_character_split_across_chunks() {
        let input = "data: {\"t\":\"\u{4f60}\u{597d}\u{1F600}\"}\n\n".as_bytes();
        for size in 1..9 {
            let evs = parse_all(input, size);
            assert_eq!(evs.len(), 1);
            assert_eq!(evs[0].data, "{\"t\":\"\u{4f60}\u{597d}\u{1F600}\"}");
        }
    }

    #[test]
    fn done_sentinel_and_spaces() {
        let evs = parse_all(b"data:no-space\n\ndata:  two-spaces\n\ndata: [DONE]\n\n", 3);
        assert_eq!(evs[0].data, "no-space");
        assert_eq!(evs[1].data, " two-spaces");
        assert!(evs[2].is_done());
    }

    #[test]
    fn trailing_event_without_blank_line_is_flushed_but_cut_line_is_not() {
        let evs = parse_all(b"data: one\n\ndata: two\n", 4);
        assert_eq!(evs.len(), 2);
        let evs = parse_all(b"data: one\n\ndata: tw", 4);
        assert_eq!(evs.len(), 1);
    }

    #[test]
    fn bom_blank_fields_and_events_without_data() {
        let evs = parse_all(
            b"\xEF\xBB\xBFdata: a\n\nevent: x\n\nid: 7\ndata\n\nretry: 5\n\n",
            2,
        );
        // "event: x" alone has no data -> not dispatched; bare "data" is an empty data line
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].data, "a");
        assert_eq!(evs[1].data, "");
        assert_eq!(evs[1].id.as_deref(), Some("7"));
    }

    #[test]
    fn limits_are_enforced() {
        let mut p = SseParser::new();
        let big = vec![b'x'; MAX_LINE_BYTES + 1];
        assert_eq!(p.push(&big), Err(SseError::LineTooLong));
    }
}
