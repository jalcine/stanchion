//! JSON-RPC 2.0 framing over a pair of pipes.
//!
//! The protocol is the real JSON-RPC 2.0 spec rather than a bespoke encoding, so a
//! host can be written in any language. [`jsonrpsee_types`] supplies the parts where
//! conformance actually matters — request ids, the `"2.0"` version marker, and the
//! standard error codes — while the envelopes here are owned rather than borrowed,
//! because messages are read off a pipe and outlive the buffer they arrived in.
//!
//! Messages are newline-delimited: one JSON object per line. JSON escapes newlines,
//! so a payload can never split a frame, and a captured stream stays readable.
//! jsonrpsee's own transports are HTTP and WebSocket only, which is why the framing
//! lives here.

use std::io::{self, BufRead, Write};

use std::borrow::Cow;

use jsonrpsee_types::{ErrorCode, ErrorObjectOwned, Id, TwoPointZero};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value as Json;

/// Deserializes a request id without borrowing from the input.
///
/// `jsonrpsee_types::Id` borrows, which suits a server parsing a buffer it still
/// owns. Messages here are read off a pipe and outlive that buffer, so the id is
/// rebuilt as owned data while keeping the crate's type and its equality semantics.
fn owned_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Id<'static>, D::Error> {
    match Json::deserialize(deserializer)? {
        Json::Null => Ok(Id::Null),
        Json::String(text) => Ok(Id::Str(Cow::Owned(text))),
        Json::Number(number) => number
            .as_u64()
            .map(Id::Number)
            .ok_or_else(|| serde::de::Error::custom("a request id must be a whole number")),
        other => Err(serde::de::Error::custom(format!(
            "a request id must be a number, string or null, found {other}"
        ))),
    }
}

/// Largest single frame accepted, to bound memory on a hostile stream.
pub const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

/// A JSON-RPC request or notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: TwoPointZero,
    #[serde(deserialize_with = "owned_id")]
    pub id: Id<'static>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Json>,
}

impl Request {
    /// Builds a request with a numeric id.
    pub fn new(id: u64, method: impl Into<String>, params: Json) -> Self {
        Request {
            jsonrpc: TwoPointZero,
            id: Id::Number(id),
            method: method.into(),
            params: Some(params),
        }
    }
}

/// A JSON-RPC response: exactly one of `result` or `error`, per the spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: TwoPointZero,
    #[serde(deserialize_with = "owned_id")]
    pub id: Id<'static>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Json>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObjectOwned>,
}

impl Response {
    /// A successful reply.
    pub fn ok(id: Id<'static>, result: Json) -> Self {
        Response {
            jsonrpc: TwoPointZero,
            id,
            result: Some(result),
            error: None,
        }
    }

    /// A failed reply.
    pub fn failed(id: Id<'static>, error: ErrorObjectOwned) -> Self {
        Response {
            jsonrpc: TwoPointZero,
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// Either direction's traffic, demultiplexed on arrival.
///
/// JSON-RPC distinguishes the two by shape rather than a tag: a request carries
/// `method`, a response carries `result` or `error`.
#[derive(Debug, Clone)]
pub enum Incoming {
    Request(Request),
    Response(Response),
}

/// Builds a standard error object.
pub fn error(code: ErrorCode, message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(code.code(), message.into(), None::<()>)
}

/// Builds an application-defined error object.
pub fn app_error(code: i32, message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(code, message.into(), None::<()>)
}

/// Writes one message as a single line.
pub fn write<W: Write, T: Serialize>(writer: &mut W, message: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(message).map_err(io::Error::other)?;
    if line.len() > MAX_FRAME_BYTES {
        return Err(io::Error::other("message exceeds the frame limit"));
    }
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()
}

/// Reads one message, or `None` at a clean end of stream.
///
/// Blank lines are skipped so a stream stays readable when something echoes one.
pub fn read<R: BufRead>(reader: &mut R) -> io::Result<Option<Incoming>> {
    let mut line = String::new();
    loop {
        line.clear();
        let read = read_limited(reader, &mut line)?;
        if read == 0 {
            return Ok(None);
        }
        if line.trim().is_empty() {
            continue;
        }

        let value: Json = serde_json::from_str(&line).map_err(io::Error::other)?;
        // Shape, not a tag: that is how JSON-RPC separates the two.
        let has_method = value.get("method").is_some();
        let has_reply = value.get("result").is_some() || value.get("error").is_some();

        let message = match (has_method, has_reply) {
            (true, false) => Incoming::Request(serde_json::from_value(value).map_err(io::Error::other)?),
            (false, true) => Incoming::Response(serde_json::from_value(value).map_err(io::Error::other)?),
            (true, true) => return Err(io::Error::other("message carries both `method` and `result`/`error`")),
            (false, false) => return Err(io::Error::other("message is neither a request nor a response")),
        };
        return Ok(Some(message));
    }
}

/// Reads a line, refusing one long enough to exhaust memory.
fn read_limited<R: BufRead>(reader: &mut R, out: &mut String) -> io::Result<usize> {
    let mut total = 0usize;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(total);
        }
        match available.iter().position(|byte| *byte == b'\n') {
            Some(end) => {
                let chunk = available.get(..=end).unwrap_or_default();
                out.push_str(&String::from_utf8_lossy(chunk));
                let consumed = end.saturating_add(1);
                reader.consume(consumed);
                return Ok(total.saturating_add(consumed));
            }
            None => {
                let len = available.len();
                out.push_str(&String::from_utf8_lossy(available));
                reader.consume(len);
                total = total.saturating_add(len);
                if total > MAX_FRAME_BYTES {
                    return Err(io::Error::other("frame exceeds the limit"));
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn a_request_round_trips_and_carries_the_version_marker() {
        let mut buffer = Vec::new();
        write(
            &mut buffer,
            &Request::new(7, "plugins/call", serde_json::json!({"a": 1})),
        )
        .unwrap();

        // Spec conformance is visible on the wire, not just in our types.
        let text = String::from_utf8(buffer.clone()).unwrap();
        assert!(text.contains(r#""jsonrpc":"2.0""#), "{text}");
        assert!(text.ends_with('\n'));

        let mut cursor = io::Cursor::new(buffer);
        match read(&mut cursor).unwrap().unwrap() {
            Incoming::Request(request) => {
                assert_eq!(request.id, Id::Number(7));
                assert_eq!(request.method, "plugins/call");
            }
            other => panic!("expected a request, got {other:?}"),
        }
    }

    #[test]
    fn requests_and_responses_are_told_apart_by_shape() {
        let mut buffer = Vec::new();
        write(&mut buffer, &Request::new(1, "host/info", Json::Null)).unwrap();
        write(
            &mut buffer,
            &Response::ok(Id::Number(1), Json::from("done")),
        )
        .unwrap();
        write(
            &mut buffer,
            &Response::failed(Id::Number(2), error(ErrorCode::MethodNotFound, "nope")),
        )
        .unwrap();

        let mut cursor = io::Cursor::new(buffer);
        assert!(matches!(
            read(&mut cursor).unwrap().unwrap(),
            Incoming::Request(_)
        ));

        match read(&mut cursor).unwrap().unwrap() {
            Incoming::Response(response) => {
                assert_eq!(response.result, Some(Json::from("done")));
                assert!(response.error.is_none());
            }
            other => panic!("expected a response, got {other:?}"),
        }

        match read(&mut cursor).unwrap().unwrap() {
            Incoming::Response(response) => {
                let error = response.error.unwrap();
                assert_eq!(error.code(), ErrorCode::MethodNotFound.code());
                assert!(response.result.is_none());
            }
            other => panic!("expected a response, got {other:?}"),
        }

        assert!(
            read(&mut cursor).unwrap().is_none(),
            "a clean end is not an error"
        );
    }

    #[test]
    fn a_wrong_version_marker_is_refused() {
        let mut cursor = io::Cursor::new(br#"{"jsonrpc":"1.0","id":1,"method":"x"}"#.to_vec());
        assert!(read(&mut cursor).is_err(), "only 2.0 is this protocol");
    }

    #[test]
    fn malformed_json_is_an_error_not_a_silent_skip() {
        let mut cursor = io::Cursor::new(b"{not json}\n".to_vec());
        assert!(read(&mut cursor).is_err());
    }

    #[test]
    fn blank_lines_are_skipped() {
        let mut buffer = b"\n\n".to_vec();
        write(&mut buffer, &Request::new(3, "host/info", Json::Null)).unwrap();
        let mut cursor = io::Cursor::new(buffer);
        assert!(matches!(
            read(&mut cursor).unwrap().unwrap(),
            Incoming::Request(_)
        ));
    }
}
