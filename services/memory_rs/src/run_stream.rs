//! The `/v1/run` streaming wire protocol.
//!
//! A successful `/v1/run` response (`Content-Type:` [`RUN_CONTENT_TYPE`])
//! carries a sequence of length-prefixed binary frames rather than JSON, so
//! arbitrary (including non-UTF-8) stdout/stderr bytes can be preserved
//! exactly and so a slow reader applies real backpressure instead of the
//! server buffering unboundedly. Binary framing was chosen over a
//! line-oriented or base64/JSON-lines encoding because it avoids
//! per-chunk encoding overhead and never requires the child's bytes to be
//! valid UTF-8 or escaped.
//!
//! Wire format (all multi-byte integers big-endian):
//!
//! ```text
//! frame := tag:u8 length:u32 payload:[u8; length]
//! ```
//!
//! | tag    | meaning               | payload                    |
//! |--------|-----------------------|-----------------------------|
//! | `0x00` | stdout chunk          | raw stdout bytes (may repeat) |
//! | `0x01` | stderr chunk          | raw stderr bytes (may repeat) |
//! | `0x02` | exited (terminal)     | `exit_code: i32`             |
//! | `0x03` | timed out (terminal)  | empty                        |
//! | `0x04` | output limit (terminal) | empty                      |
//! | `0x05` | cancelled (terminal)  | empty                        |
//! | `0x06` | launch failed (terminal) | UTF-8 failure message    |
//! | `0x07` | not allowed (terminal) | UTF-8 rejected command name |
//!
//! Exactly one terminal frame (`0x02`-`0x07`) ends a well-formed stream.
//! Stdout/stderr chunk frames may appear any number of times, in any
//! interleaving, before it. A stream that ends (the HTTP body reaches EOF)
//! without having produced a terminal frame is malformed/incomplete and
//! must be reported as [`MemoryError::MalformedResponse`], never treated as
//! a successful, merely-quiet command.
//!
//! In practice the Axum adapter (`crate::server`) validates the requested
//! executable against the allowlist before it ever opens a stream, so
//! `0x07` is not observed from that adapter; it is still part of the wire
//! format (and encoded/decoded symmetrically) so a decoder never has to
//! assume which terminal outcomes are reachable.

use crate::{command_runner::RunOutcome, error::MemoryError};

/// The content type of a successful `/v1/run` response.
pub const RUN_CONTENT_TYPE: &str = "application/vnd.agentspace.memory-run";

/// The frame header size in bytes: one tag byte plus a big-endian `u32`
/// length.
pub const FRAME_HEADER_LEN: usize = 5;

/// The largest payload this build will ever place in one frame.
///
/// A frame claiming a longer payload is rejected as malformed rather than
/// trusted, bounding how much a reader must buffer for one frame regardless
/// of what a peer claims.
pub const MAX_FRAME_PAYLOAD_BYTES: u32 = 1024 * 1024;

pub const TAG_STDOUT: u8 = 0x00;
pub const TAG_STDERR: u8 = 0x01;
pub const TAG_EXITED: u8 = 0x02;
pub const TAG_TIMED_OUT: u8 = 0x03;
pub const TAG_OUTPUT_LIMIT_EXCEEDED: u8 = 0x04;
pub const TAG_CANCELLED: u8 = 0x05;
pub const TAG_LAUNCH_FAILED: u8 = 0x06;
pub const TAG_NOT_ALLOWED: u8 = 0x07;

/// One decoded `/v1/run` stream frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunFrame {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Terminal(RunOutcome),
}

impl RunFrame {
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminal(_))
    }
}

/// Encodes one stdout or stderr chunk frame. `tag` must be
/// [`TAG_STDOUT`] or [`TAG_STDERR`].
///
/// A payload longer than [`MAX_FRAME_PAYLOAD_BYTES`] is truncated rather
/// than panicking or silently growing the frame past the bound every
/// reader enforces; callers always write well below that bound (the
/// server writes at most its internal copy-buffer size per frame), so this
/// only guards against a future caller regression.
#[must_use]
pub fn encode_chunk(tag: u8, payload: &[u8]) -> Vec<u8> {
    let max_len = MAX_FRAME_PAYLOAD_BYTES as usize;
    let payload = if payload.len() > max_len {
        &payload[..max_len]
    } else {
        payload
    };
    let len = u32::try_from(payload.len()).unwrap_or(MAX_FRAME_PAYLOAD_BYTES);
    let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
    frame.push(tag);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Encodes the single terminal frame that ends a `/v1/run` stream.
#[must_use]
pub fn encode_terminal(outcome: &RunOutcome) -> Vec<u8> {
    let (tag, payload): (u8, Vec<u8>) = match outcome {
        RunOutcome::Exited(code) => (TAG_EXITED, code.to_be_bytes().to_vec()),
        RunOutcome::TimedOut => (TAG_TIMED_OUT, Vec::new()),
        RunOutcome::OutputLimitExceeded => (TAG_OUTPUT_LIMIT_EXCEEDED, Vec::new()),
        RunOutcome::Cancelled => (TAG_CANCELLED, Vec::new()),
        RunOutcome::LaunchFailed(message) => (TAG_LAUNCH_FAILED, message.clone().into_bytes()),
        RunOutcome::NotAllowed(command) => (TAG_NOT_ALLOWED, command.clone().into_bytes()),
    };
    let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
    frame.push(tag);
    frame.extend_from_slice(
        &u32::try_from(payload.len())
            .unwrap_or(MAX_FRAME_PAYLOAD_BYTES)
            .to_be_bytes(),
    );
    frame.extend_from_slice(&payload);
    frame
}

/// Incrementally decodes a byte stream into [`RunFrame`]s as complete
/// frames become available, buffering only the bytes of one incomplete
/// frame (bounded by [`MAX_FRAME_PAYLOAD_BYTES`]) between calls.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
    terminated: bool,
}

impl FrameDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends newly received bytes to the internal buffer.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Returns whether a terminal frame has already been decoded; once
    /// true, [`Self::next_frame`] will not decode any further frames.
    #[must_use]
    pub const fn terminated(&self) -> bool {
        self.terminated
    }

    /// Decodes and removes the next complete frame from the buffer, if any.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::MalformedResponse`] if the buffered header
    /// declares a payload longer than [`MAX_FRAME_PAYLOAD_BYTES`] or names
    /// an unrecognized tag.
    pub fn next_frame(&mut self) -> Result<Option<RunFrame>, MemoryError> {
        if self.terminated || self.buffer.len() < FRAME_HEADER_LEN {
            return Ok(None);
        }
        let tag = self.buffer[0];
        let mut length_bytes = [0_u8; 4];
        length_bytes.copy_from_slice(&self.buffer[1..FRAME_HEADER_LEN]);
        let length = u32::from_be_bytes(length_bytes);
        if length > MAX_FRAME_PAYLOAD_BYTES {
            return Err(MemoryError::malformed_response(format!(
                "run stream frame declared a {length}-byte payload, exceeding the {MAX_FRAME_PAYLOAD_BYTES}-byte limit"
            )));
        }
        let length = length as usize;
        let total = FRAME_HEADER_LEN + length;
        if self.buffer.len() < total {
            return Ok(None);
        }

        let payload = self.buffer[FRAME_HEADER_LEN..total].to_vec();
        self.buffer.drain(..total);

        let frame = match tag {
            TAG_STDOUT => RunFrame::Stdout(payload),
            TAG_STDERR => RunFrame::Stderr(payload),
            TAG_EXITED => {
                let code = decode_i32(&payload)?;
                RunFrame::Terminal(RunOutcome::Exited(code))
            }
            TAG_TIMED_OUT => RunFrame::Terminal(RunOutcome::TimedOut),
            TAG_OUTPUT_LIMIT_EXCEEDED => RunFrame::Terminal(RunOutcome::OutputLimitExceeded),
            TAG_CANCELLED => RunFrame::Terminal(RunOutcome::Cancelled),
            TAG_LAUNCH_FAILED => {
                let message = decode_utf8(payload)?;
                RunFrame::Terminal(RunOutcome::LaunchFailed(message))
            }
            TAG_NOT_ALLOWED => {
                let command = decode_utf8(payload)?;
                RunFrame::Terminal(RunOutcome::NotAllowed(command))
            }
            other => {
                return Err(MemoryError::malformed_response(format!(
                    "run stream frame used unrecognized tag {other:#04x}"
                )));
            }
        };
        if frame.is_terminal() {
            self.terminated = true;
        }
        Ok(Some(frame))
    }
}

fn decode_i32(payload: &[u8]) -> Result<i32, MemoryError> {
    let bytes: [u8; 4] = payload.try_into().map_err(|_error| {
        MemoryError::malformed_response(format!(
            "run stream exit frame carried {} bytes, expected 4",
            payload.len()
        ))
    })?;
    Ok(i32::from_be_bytes(bytes))
}

fn decode_utf8(payload: Vec<u8>) -> Result<String, MemoryError> {
    String::from_utf8(payload).map_err(|error| {
        MemoryError::malformed_response(format!("run stream frame carried invalid UTF-8: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        FrameDecoder, MAX_FRAME_PAYLOAD_BYTES, RunFrame, TAG_STDOUT, encode_chunk, encode_terminal,
    };
    use crate::command_runner::RunOutcome;

    #[test]
    fn round_trips_interleaved_stdout_stderr_and_exit() {
        let mut wire = Vec::new();
        wire.extend(encode_chunk(TAG_STDOUT, b"out-1"));
        wire.extend(encode_chunk(super::TAG_STDERR, b"err-1"));
        wire.extend(encode_chunk(TAG_STDOUT, b"out-2"));
        wire.extend(encode_terminal(&RunOutcome::Exited(7)));

        let mut decoder = FrameDecoder::new();
        decoder.push(&wire);
        let mut frames = Vec::new();
        while let Some(frame) = decoder
            .next_frame()
            .unwrap_or_else(|error| panic!("{error}"))
        {
            frames.push(frame);
        }

        assert_eq!(
            frames,
            vec![
                RunFrame::Stdout(b"out-1".to_vec()),
                RunFrame::Stderr(b"err-1".to_vec()),
                RunFrame::Stdout(b"out-2".to_vec()),
                RunFrame::Terminal(RunOutcome::Exited(7)),
            ]
        );
        assert!(decoder.terminated());
    }

    #[test]
    fn decodes_incrementally_across_partial_chunks() {
        let frame = encode_chunk(TAG_STDOUT, b"hello");
        let mut decoder = FrameDecoder::new();

        decoder.push(&frame[..3]);
        assert_eq!(
            decoder
                .next_frame()
                .unwrap_or_else(|error| panic!("{error}")),
            None
        );

        decoder.push(&frame[3..]);
        assert_eq!(
            decoder
                .next_frame()
                .unwrap_or_else(|error| panic!("{error}")),
            Some(RunFrame::Stdout(b"hello".to_vec()))
        );
    }

    #[test]
    fn preserves_arbitrary_non_utf8_bytes() {
        let payload = vec![0_u8, 255, 1, 128, 254];
        let frame = encode_chunk(TAG_STDOUT, &payload);
        let mut decoder = FrameDecoder::new();
        decoder.push(&frame);
        assert_eq!(
            decoder
                .next_frame()
                .unwrap_or_else(|error| panic!("{error}")),
            Some(RunFrame::Stdout(payload))
        );
    }

    #[test]
    fn rejects_oversized_declared_length() {
        let mut header = vec![TAG_STDOUT];
        header.extend_from_slice(&(MAX_FRAME_PAYLOAD_BYTES + 1).to_be_bytes());
        let mut decoder = FrameDecoder::new();
        decoder.push(&header);
        let error = decoder
            .next_frame()
            .map_or_else(|error| error, |_| panic!("must reject oversized frame"));
        assert!(matches!(
            error,
            crate::error::MemoryError::MalformedResponse { .. }
        ));
    }

    #[test]
    fn rejects_unrecognized_tag() {
        let frame = encode_chunk(TAG_STDOUT, b"");
        let mut corrupted = frame;
        corrupted[0] = 0x7F;
        let mut decoder = FrameDecoder::new();
        decoder.push(&corrupted);
        let error = decoder
            .next_frame()
            .map_or_else(|error| error, |_| panic!("must reject unknown tag"));
        assert!(matches!(
            error,
            crate::error::MemoryError::MalformedResponse { .. }
        ));
    }

    #[test]
    fn no_terminal_frame_leaves_decoder_unterminated() {
        let mut decoder = FrameDecoder::new();
        decoder.push(&encode_chunk(TAG_STDOUT, b"partial-only"));
        let _ = decoder.next_frame();
        assert!(!decoder.terminated());
    }

    #[test]
    fn round_trips_launch_failed_and_not_allowed_terminals() {
        for outcome in [
            RunOutcome::LaunchFailed("no such file or directory".to_owned()),
            RunOutcome::NotAllowed("rm".to_owned()),
        ] {
            let mut decoder = FrameDecoder::new();
            decoder.push(&encode_terminal(&outcome));
            assert_eq!(
                decoder
                    .next_frame()
                    .unwrap_or_else(|error| panic!("{error}")),
                Some(RunFrame::Terminal(outcome))
            );
        }
    }
}
