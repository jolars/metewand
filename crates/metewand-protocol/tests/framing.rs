use std::io::{self, Read};

use metewand_protocol::framing::{
    CorrelationError, FrameError, FrameReader, MAX_LINE_BYTES, ResponseCorrelator,
};
use serde_json::json;

struct Fragmented<'a> {
    remaining: &'a [u8],
    fragment_lengths: &'a [usize],
    next_fragment: usize,
}

impl<'a> Fragmented<'a> {
    fn new(remaining: &'a [u8], fragment_lengths: &'a [usize]) -> Self {
        Self {
            remaining,
            fragment_lengths,
            next_fragment: 0,
        }
    }
}

impl Read for Fragmented<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.remaining.is_empty() {
            return Ok(0);
        }

        let requested = self.fragment_lengths[self.next_fragment % self.fragment_lengths.len()];
        self.next_fragment += 1;
        let length = requested.min(output.len()).min(self.remaining.len());
        output[..length].copy_from_slice(&self.remaining[..length]);
        self.remaining = &self.remaining[length..];
        Ok(length)
    }
}

#[test]
fn reconstructs_frames_across_fragmented_reads() {
    let input = br#"{"id":"request-1","ok":true}
{"id":"request-2","ok":false}
"#;
    let fragmented = Fragmented::new(input, &[1, 2, 1, 5, 3]);
    let mut reader = FrameReader::new(fragmented);

    assert_eq!(
        reader.read_frame().expect("first frame must decode"),
        Some(json!({"id": "request-1", "ok": true}))
    );
    assert_eq!(
        reader.read_frame().expect("second frame must decode"),
        Some(json!({"id": "request-2", "ok": false}))
    );
    assert_eq!(reader.read_frame().expect("clean EOF must decode"), None);
}

#[test]
fn rejects_duplicate_keys_at_any_nesting_depth() {
    let mut reader = FrameReader::new(
        &br#"{"id":"request-1","result":{"value":1,"value":2}}
"#[..],
    );

    let error = reader.read_frame().expect_err("duplicate key must fail");
    assert!(matches!(
        error,
        FrameError::DuplicateKey { ref key } if key == "value"
    ));
}

#[test]
fn rejects_invalid_utf8() {
    let mut reader = FrameReader::new(&b"{\"id\":\"request-\xff\"}\n"[..]);

    let error = reader.read_frame().expect_err("invalid UTF-8 must fail");
    assert!(matches!(error, FrameError::InvalidUtf8 { .. }));
}

#[test]
fn rejects_a_utf8_byte_order_mark() {
    let mut reader = FrameReader::new(&b"\xef\xbb\xbf{\"id\":\"request-1\"}\n"[..]);

    let error = reader
        .read_frame()
        .expect_err("a byte-order mark must fail");
    assert!(matches!(error, FrameError::ByteOrderMark));
}

#[test]
fn rejects_a_line_larger_than_one_mebibyte_without_unbounded_buffering() {
    let mut input = vec![b' '; MAX_LINE_BYTES];
    input.push(b'\n');
    assert_eq!(input.len(), MAX_LINE_BYTES + 1);
    let mut reader = FrameReader::new(input.as_slice());

    let error = reader
        .read_frame()
        .expect_err("an oversized line must fail");
    assert!(matches!(
        error,
        FrameError::LineTooLong {
            limit: MAX_LINE_BYTES
        }
    ));
}

#[test]
fn accepts_a_message_at_the_exact_line_limit() {
    let padding_length = MAX_LINE_BYTES - b"{\"padding\":\"\"}\n".len();
    let mut input = Vec::with_capacity(MAX_LINE_BYTES);
    input.extend_from_slice(b"{\"padding\":\"");
    input.extend(std::iter::repeat_n(b'a', padding_length));
    input.extend_from_slice(b"\"}\n");
    assert_eq!(input.len(), MAX_LINE_BYTES);

    let mut reader = FrameReader::new(input.as_slice());
    assert!(
        reader
            .read_frame()
            .expect("limit must be inclusive")
            .is_some()
    );
}

#[test]
fn rejects_malformed_json() {
    let mut reader = FrameReader::new(&b"{\"id\":}\n"[..]);

    let error = reader.read_frame().expect_err("malformed JSON must fail");
    assert!(matches!(error, FrameError::MalformedJson { .. }));
}

#[test]
fn rejects_early_eof_before_the_required_newline() {
    let mut reader = FrameReader::new(&br#"{"id":"request-1"}"#[..]);

    let error = reader.read_frame().expect_err("partial line must fail");
    assert!(matches!(
        error,
        FrameError::EarlyEof { received } if received == br#"{"id":"request-1"}"#.len()
    ));
}

#[test]
fn rejects_an_extra_response_after_the_pending_request_is_satisfied() {
    let input = br#"{"id":"request-1","ok":true}
{"id":"request-1","ok":true}
"#;
    let mut reader = FrameReader::new(&input[..]);
    let mut correlator = ResponseCorrelator::new("request-1");
    correlator
        .accept(
            &reader
                .read_frame()
                .expect("first frame must decode")
                .expect("first frame must exist"),
        )
        .expect("the pending response must match");

    let error = correlator
        .accept(
            &reader
                .read_frame()
                .expect("second frame must decode")
                .expect("second frame must exist"),
        )
        .expect_err("a second response must fail");
    assert!(matches!(
        error,
        CorrelationError::ExtraResponse { ref id } if id == "request-1"
    ));
}

#[test]
fn rejects_a_response_with_a_mismatched_request_id() {
    let mut reader = FrameReader::new(
        &br#"{"id":"request-2","ok":true}
"#[..],
    );
    let mut correlator = ResponseCorrelator::new("request-1");

    let error = correlator
        .accept(
            &reader
                .read_frame()
                .expect("response frame must decode")
                .expect("response frame must exist"),
        )
        .expect_err("a mismatched response must fail");
    assert!(matches!(
        error,
        CorrelationError::MismatchedRequestId {
            ref expected,
            ref actual,
        } if expected == "request-1" && actual == "request-2"
    ));
}

#[test]
fn rejects_a_response_without_a_string_request_id() {
    for response in [json!({"ok": true}), json!({"id": 1, "ok": true})] {
        let mut correlator = ResponseCorrelator::new("request-1");
        let error = correlator
            .accept(&response)
            .expect_err("a string request ID is required");
        assert!(matches!(error, CorrelationError::InvalidRequestId));
    }
}

#[test]
fn preserves_the_pending_request_after_a_mismatched_id() {
    let mut correlator = ResponseCorrelator::new("request-1");
    correlator
        .accept(&json!({"id": "late-request", "ok": true}))
        .expect_err("a mismatched response must fail");

    correlator
        .accept(&json!({"id": "request-1", "ok": true}))
        .expect("the expected response must remain pending");
    assert!(correlator.is_complete());
}
