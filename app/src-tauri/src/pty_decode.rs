//! Incremental UTF-8 reassembly for PTY / SSH byte streams.
//!
//! PTY output arrives in arbitrary-length chunks, so a multi-byte
//! character (Hebrew, emoji, box-drawing) is routinely split across two
//! reads. The decoder therefore holds an incomplete trailing sequence
//! back until the next chunk completes it. Decoding each chunk on its
//! own with `from_utf8_lossy` would turn every split Hebrew letter into
//! U+FFFD — the mojibake class of bug fixed in `1bcb584`, which is why
//! the "just make it lossy" shortcut is NOT what this module does.
//!
//! The other half of the problem is bytes that are not a truncated
//! sequence but genuine garbage: a `cat` on a binary file, a legacy
//! console app writing raw CP1255, a corrupted download. A byte like
//! `0x80` (a bare continuation byte) can never *begin* a valid sequence,
//! so waiting for more bytes to complete it waits forever. The two cases
//! are told apart by `Utf8Error::error_len()`:
//!
//!   * `None`    — input ended mid-sequence; the tail may still become
//!                 valid once more bytes arrive. Hold it.
//!   * `Some(n)` — `n` bytes are invalid no matter what follows. Emit
//!                 U+FFFD, skip past them, keep going.
//!
//! Before that distinction existed, an invalid *leading* byte pinned
//! `valid_up_to()` at 0 permanently: nothing was ever drained, so the
//! pane went silent forever and the buffer grew by one read per read
//! with no cap. Draining at least one byte on every `Some(n)` is what
//! makes a stall structurally impossible now.

/// The result of feeding one chunk through a [`Utf8Stream`].
pub struct Decoded {
    /// Everything decodable so far. May be empty when the whole chunk
    /// was a truncated character being carried to the next read.
    pub text: String,
    /// `Some(n)` exactly once per stream — the first chunk in which
    /// invalid bytes had to be dropped, and how many. Later drops report
    /// `None` so a pane dumping binary logs one line, not thousands.
    pub first_invalid: Option<usize>,
}

/// Per-stream decode state. One instance per byte stream, never shared:
/// splicing two streams' bytes into one reassembly buffer would join the
/// tail of one character to the head of another.
#[derive(Default)]
pub struct Utf8Stream {
    /// Trailing bytes of an incomplete multi-byte character, carried
    /// into the next chunk. Bounded by construction — `push` only ever
    /// leaves a *truncated valid prefix* here, and UTF-8 caps that at 3
    /// bytes (the first 3 of a 4-byte codepoint). See the
    /// `leftover_never_exceeds_three_bytes` test.
    leftover: Vec<u8>,
    /// Set once an invalid-byte event has been reported for this stream.
    reported: bool,
}

impl Utf8Stream {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds `bytes` and returns everything that is now decodable.
    pub fn push(&mut self, bytes: &[u8]) -> Decoded {
        self.leftover.extend_from_slice(bytes);
        let mut text = String::with_capacity(self.leftover.len());
        let mut dropped: usize = 0;

        loop {
            // Bind plain numbers rather than the `&str` so the borrow on
            // `leftover` ends before we drain it.
            let (valid, err_len) = match std::str::from_utf8(&self.leftover) {
                Ok(_) => (self.leftover.len(), None),
                Err(e) => (e.valid_up_to(), e.error_len()),
            };
            if valid > 0 {
                // The prefix is known-valid, so `from_utf8_lossy` is an
                // exact decode here — and it avoids an `unwrap` (Rule #4).
                text.push_str(&String::from_utf8_lossy(&self.leftover[..valid]));
            }
            match err_len {
                // Either the whole buffer decoded (`valid == len`, so this
                // drains it empty) or the tail is a truncated-but-plausible
                // sequence we carry into the next chunk.
                None => {
                    self.leftover.drain(..valid);
                    break;
                }
                // Genuinely invalid: substitute and step past. `valid + n`
                // is always >= 1, which is the anti-stall guarantee.
                Some(n) => {
                    text.push('\u{FFFD}');
                    dropped += n;
                    self.leftover.drain(..valid + n);
                }
            }
        }

        let first_invalid = if dropped > 0 && !self.reported {
            self.reported = true;
            Some(dropped)
        } else {
            None
        };
        Decoded { text, first_invalid }
    }
}

// ─── tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds every chunk and concatenates the decoded text.
    fn feed(s: &mut Utf8Stream, chunks: &[&[u8]]) -> String {
        let mut out = String::new();
        for c in chunks {
            out.push_str(&s.push(c).text);
        }
        out
    }

    #[test]
    fn plain_ascii_round_trips() {
        let mut s = Utf8Stream::new();
        assert_eq!(feed(&mut s, &[b"hello world"]), "hello world");
        assert!(s.leftover.is_empty());
    }

    #[test]
    fn empty_chunk_is_a_noop() {
        let mut s = Utf8Stream::new();
        let d = s.push(b"");
        assert_eq!(d.text, "");
        assert!(d.first_invalid.is_none());
    }

    /// The regression `1bcb584` fixed: a Hebrew letter split across two
    /// reads must NOT become U+FFFD.
    #[test]
    fn hebrew_split_across_chunks_survives() {
        let text = "שלום עולם";
        let bytes = text.as_bytes();
        // Split at every byte boundary; each split must reassemble.
        for cut in 0..bytes.len() {
            let mut s = Utf8Stream::new();
            let got = feed(&mut s, &[&bytes[..cut], &bytes[cut..]]);
            assert_eq!(got, text, "split at {cut}");
            assert!(!got.contains('\u{FFFD}'), "split at {cut} produced U+FFFD");
            assert!(s.leftover.is_empty(), "split at {cut} left residue");
        }
    }

    #[test]
    fn four_byte_emoji_split_at_every_boundary() {
        let text = "a🎉b";
        let bytes = text.as_bytes();
        for cut in 0..bytes.len() {
            let mut s = Utf8Stream::new();
            assert_eq!(feed(&mut s, &[&bytes[..cut], &bytes[cut..]]), text, "split at {cut}");
        }
    }

    #[test]
    fn byte_at_a_time_reassembles() {
        let text = "דוח-DEV.txt 🎉";
        let mut s = Utf8Stream::new();
        let mut out = String::new();
        for b in text.as_bytes() {
            out.push_str(&s.push(&[*b]).text);
        }
        assert_eq!(out, text);
    }

    /// THE BUG: a bare continuation byte can never start a sequence, so
    /// the old decoder returned with `valid_up_to == 0` and never drained
    /// — the pane went silent forever. It must now drain and keep going.
    #[test]
    fn leading_continuation_byte_does_not_stall_the_stream() {
        let mut s = Utf8Stream::new();
        let first = s.push(&[0x80]);
        assert_eq!(first.text, "\u{FFFD}");
        assert_eq!(first.first_invalid, Some(1));
        assert!(s.leftover.is_empty(), "invalid byte was not drained");
        // The stream must still be live afterwards.
        assert_eq!(s.push(b"back alive").text, "back alive");
    }

    /// Same shape, with the bytes that a `cat` on a binary file produces.
    #[test]
    fn invalid_lead_bytes_do_not_stall_the_stream() {
        for bad in [0x80u8, 0xBF, 0xC0, 0xC1, 0xF5, 0xFE, 0xFF] {
            let mut s = Utf8Stream::new();
            let d = s.push(&[bad]);
            assert!(d.text.contains('\u{FFFD}'), "byte {bad:#04x} produced no output");
            assert!(s.leftover.is_empty(), "byte {bad:#04x} was not drained");
            assert_eq!(s.push(b"ok").text, "ok", "byte {bad:#04x} left the stream dead");
        }
    }

    #[test]
    fn invalid_byte_mid_chunk_keeps_surrounding_text() {
        let mut s = Utf8Stream::new();
        let d = s.push(b"abc\xFFdef");
        assert_eq!(d.text, "abc\u{FFFD}def");
        assert_eq!(d.first_invalid, Some(1));
    }

    #[test]
    fn valid_text_after_garbage_still_arrives() {
        let mut s = Utf8Stream::new();
        // Garbage chunk, then real Hebrew — the pane must recover.
        s.push(&[0xFF, 0xFE, 0x80]);
        assert_eq!(s.push("שלום".as_bytes()).text, "שלום");
    }

    /// A truncated sequence is at most 3 bytes, so the buffer can never
    /// grow without bound — the second half of the original bug.
    #[test]
    fn leftover_never_exceeds_three_bytes() {
        let mut s = Utf8Stream::new();
        // 8 KiB of random-ish garbage, the size of one PTY read.
        let junk: Vec<u8> = (0..8192u32).map(|i| (i.wrapping_mul(37) % 256) as u8).collect();
        for _ in 0..64 {
            s.push(&junk);
            assert!(s.leftover.len() <= 3, "leftover grew to {}", s.leftover.len());
        }
    }

    #[test]
    fn invalid_is_reported_once_per_stream() {
        let mut s = Utf8Stream::new();
        assert_eq!(s.push(&[0xFF]).first_invalid, Some(1));
        assert_eq!(s.push(&[0xFF]).first_invalid, None);
        assert_eq!(s.push(&[0xFF, 0xFE]).first_invalid, None);
    }

    #[test]
    fn report_counts_all_bytes_dropped_in_the_first_chunk() {
        let mut s = Utf8Stream::new();
        // Three separate invalid runs in one chunk.
        assert_eq!(s.push(b"\xFFa\xFEb\x80").first_invalid, Some(3));
    }

    /// A truncated tail is held, not reported as invalid — otherwise
    /// every split Hebrew char would log a warning.
    #[test]
    fn truncated_tail_is_not_an_invalid_report() {
        let mut s = Utf8Stream::new();
        let d = s.push(&"ש".as_bytes()[..1]);
        assert_eq!(d.text, "");
        assert_eq!(d.first_invalid, None);
        assert_eq!(s.leftover.len(), 1);
    }

    /// Two streams must not share state: stdout's truncated tail must not
    /// be completed by stderr's bytes.
    #[test]
    fn separate_streams_do_not_splice() {
        let heb = "ש".as_bytes();
        let mut out = Utf8Stream::new();
        let mut err = Utf8Stream::new();
        out.push(&heb[..1]);
        // stderr's own bytes decode independently of stdout's pending tail.
        assert_eq!(err.push(b"E: failed").text, "E: failed");
        assert_eq!(out.push(&heb[1..]).text, "ש");
    }
}
