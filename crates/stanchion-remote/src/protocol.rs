//! Wire protocol for the out-of-process plugin host.
//!
//! Messages are length-prefixed JSON: a little-endian `u32` byte count followed by
//! that many bytes of UTF-8 JSON. Length framing rather than newline delimiting so a
//! payload never has to be escaped or scanned, and JSON rather than a compact binary
//! encoding because the channel is a debugging surface as much as a transport — you
//! can read a captured stream without tooling.
//!
//! The protocol is dynamically typed on purpose: a host binary is compiled before
//! anyone writes a plugin, so it cannot know a `#[lua_class]` trait. Arguments and
//! results cross as JSON and are converted at the Lua boundary by `mlua`'s serde
//! support.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// Largest message the codec will read, to bound memory on a hostile stream.
pub const MAX_MESSAGE_BYTES: u32 = 32 * 1024 * 1024;

/// One framed message, correlated by `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<T> {
    /// Correlates a reply with its request. Callbacks reuse the same space.
    pub id: u64,
    /// The message itself.
    #[serde(flatten)]
    pub body: T,
}

/// What the core application asks the host to do.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Discover and load every plugin under `root`.
    Load { root: String },
    /// Report the loaded plugins.
    List,
    /// Report what plugins under `root` request, without running any of their code.
    Audit { root: String },
    /// Call one method on one plugin.
    Call {
        plugin: String,
        method: String,
        #[serde(default)]
        args: Vec<Json>,
    },
    /// Call the same method on every plugin, collecting one outcome each.
    Dispatch {
        method: String,
        #[serde(default)]
        args: Vec<Json>,
    },
    /// Re-read one plugin from disk and swap in a fresh instance.
    Reload { plugin: String },
    /// Unbind a granted capability from a live plugin.
    Revoke { plugin: String, capability: String },
    /// Report the host's own version and configuration summary.
    Info,
    /// Finish serving and exit.
    Shutdown,
}

/// What the host answers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    /// Result of a `Load`.
    Loaded {
        loaded: Vec<String>,
        failures: Vec<Failure>,
    },
    /// Result of a `List`.
    ///
    /// Struct variants throughout, not newtypes: serde's internally tagged
    /// representation cannot serialize a newtype variant wrapping a sequence, so
    /// `Plugins(Vec<_>)` would fail at runtime rather than at compile time.
    Plugins { plugins: Vec<PluginInfo> },
    /// Result of an `Audit`.
    Audit { entries: Vec<AuditEntry> },
    /// Result of a `Call`.
    Value { value: Json },
    /// Result of a `Dispatch`.
    Outcomes { outcomes: Vec<Outcome> },
    /// Result of `Info`.
    Info {
        version: String,
        isolation: String,
        signatures_required: bool,
    },
    /// The request succeeded and returns nothing.
    Ok,
    /// The request failed. Plugin-level failures are reported in place instead.
    Error { message: String },
}

/// One plugin that did not load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Failure {
    pub plugin: String,
    pub reason: String,
}

/// A loaded plugin, as the core application sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: Option<String>,
    pub granted: Vec<String>,
    pub signer: String,
}

/// What one plugin requests, established without executing it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub plugin: String,
    pub capabilities: Vec<String>,
    pub signer: String,
}

/// One plugin's result from a dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub plugin: String,
    /// Present when the call succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Json>,
    /// Present when it failed. One plugin failing never affects the others.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Writes one length-prefixed JSON message.
pub fn write_message<W: Write, T: Serialize>(writer: &mut W, message: &T) -> io::Result<()> {
    let payload = serde_json::to_vec(message).map_err(io::Error::other)?;
    let length = u32::try_from(payload.len())
        .map_err(|_| io::Error::other("message exceeds the frame limit"))?;
    if length > MAX_MESSAGE_BYTES {
        return Err(io::Error::other("message exceeds the frame limit"));
    }
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()
}

/// Reads one length-prefixed JSON message, or `None` at a clean end of stream.
pub fn read_message<R: Read, T: for<'de> Deserialize<'de>>(
    reader: &mut R,
) -> io::Result<Option<T>> {
    let mut header = [0u8; 4];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        // The peer closed between messages, which is an orderly shutdown.
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }

    let length = u32::from_le_bytes(header);
    if length > MAX_MESSAGE_BYTES {
        return Err(io::Error::other("declared frame exceeds the limit"));
    }

    let mut payload = vec![0u8; length as usize];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload).map(Some).map_err(io::Error::other)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let sent = Envelope {
            id: 7,
            body: Request::Call {
                plugin: "weather".to_string(),
                method: "fetch".to_string(),
                args: vec![Json::String("oslo".to_string())],
            },
        };

        let mut buffer = Vec::new();
        write_message(&mut buffer, &sent).unwrap();

        let mut cursor = io::Cursor::new(buffer);
        let received: Envelope<Request> = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(received.id, 7);
        assert!(matches!(received.body, Request::Call { ref method, .. } if method == "fetch"));
    }

    #[test]
    fn several_messages_share_one_stream() {
        let mut buffer = Vec::new();
        for id in 0..3u64 {
            write_message(&mut buffer, &Envelope { id, body: Request::List }).unwrap();
        }

        let mut cursor = io::Cursor::new(buffer);
        for expected in 0..3u64 {
            let message: Envelope<Request> = read_message(&mut cursor).unwrap().unwrap();
            assert_eq!(message.id, expected);
        }
        // A clean end of stream is not an error.
        let end: Option<Envelope<Request>> = read_message(&mut cursor).unwrap();
        assert!(end.is_none());
    }

    /// Every variant must survive the codec. An earlier version used newtype variants
    /// here, which the internally tagged representation refuses at *runtime* — the
    /// host died mid-reply rather than failing to compile.
    #[test]
    fn every_response_variant_round_trips() {
        let responses = vec![
            Response::Loaded {
                loaded: vec!["a".to_string()],
                failures: vec![Failure {
                    plugin: "b".to_string(),
                    reason: "boom".to_string(),
                }],
            },
            Response::Plugins {
                plugins: vec![PluginInfo {
                    name: "a".to_string(),
                    version: Some("1.0.0".to_string()),
                    granted: vec!["log".to_string()],
                    signer: "unsigned".to_string(),
                }],
            },
            Response::Audit {
                entries: vec![AuditEntry {
                    plugin: "a".to_string(),
                    capabilities: vec!["log".to_string()],
                    signer: "unsigned".to_string(),
                }],
            },
            Response::Value { value: Json::Array(vec![Json::from(1), Json::from(2)]) },
            Response::Value { value: Json::Null },
            Response::Outcomes {
                outcomes: vec![Outcome {
                    plugin: "a".to_string(),
                    value: Some(Json::from("ok")),
                    error: None,
                }],
            },
            Response::Info {
                version: "0.1.0".to_string(),
                isolation: "per-plugin".to_string(),
                signatures_required: false,
            },
            Response::Ok,
            Response::Error { message: "no".to_string() },
        ];

        for (id, response) in responses.into_iter().enumerate() {
            let mut buffer = Vec::new();
            let id = id as u64;
            write_message(&mut buffer, &Envelope { id, body: response })
                .unwrap_or_else(|err| panic!("variant {id} failed to serialize: {err}"));
            let mut cursor = io::Cursor::new(buffer);
            let back: Envelope<Response> = read_message(&mut cursor).unwrap().unwrap();
            assert_eq!(back.id, id);
        }
    }

    #[test]
    fn an_oversized_frame_is_refused_without_allocating_it() {
        let mut stream = Vec::new();
        stream.extend_from_slice(&u32::MAX.to_le_bytes());
        let mut cursor = io::Cursor::new(stream);
        let err = read_message::<_, Envelope<Request>>(&mut cursor).unwrap_err();
        assert!(err.to_string().contains("exceeds the limit"), "{err}");
    }

    #[test]
    fn a_truncated_payload_is_an_error_not_a_silent_short_read() {
        let mut stream = Vec::new();
        stream.extend_from_slice(&64u32.to_le_bytes());
        stream.extend_from_slice(b"{}");
        let mut cursor = io::Cursor::new(stream);
        assert!(read_message::<_, Envelope<Request>>(&mut cursor).is_err());
    }
}
