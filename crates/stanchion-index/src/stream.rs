//! Turning a synchronous reader into an async byte stream, with bounded memory.
//!
//! [`IndexSource`](crate::IndexSource) is synchronous, matching
//! [`PluginIndex`](stanchion_dist::PluginIndex) — a package is a file, and opening a
//! file is not an async problem. Serving it to a socket is. This bridges the two: the
//! read happens on a blocking worker, and chunks arrive through a bounded channel, so
//! peak memory is the chunk size times the queue depth rather than the package.
//!
//! Both adapters use this, so there is one place where the chunking is decided.

use std::io::Read;

use bytes::Bytes;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

/// Bytes read per chunk.
pub const DEFAULT_CHUNK: usize = 64 * 1024;

/// Queued chunks. With [`DEFAULT_CHUNK`] this bounds a response to a few hundred KiB
/// in flight however large the package is.
const QUEUE_DEPTH: usize = 4;

/// Streams a reader in chunks, reading on a blocking worker.
///
/// The stream ends early with an error if the read fails part-way. That is the honest
/// outcome: the response has already begun, so the only signal left is a truncated
/// body, and a client verifying a digest will reject it.
pub fn chunks(reader: Box<dyn Read + Send>) -> ReceiverStream<std::io::Result<Bytes>> {
    chunks_of(reader, DEFAULT_CHUNK)
}

/// Streams a reader in chunks of `size`.
pub fn chunks_of(
    mut reader: Box<dyn Read + Send>,
    size: usize,
) -> ReceiverStream<std::io::Result<Bytes>> {
    let (sender, receiver) = mpsc::channel(QUEUE_DEPTH);
    let size = size.max(1);

    tokio::task::spawn_blocking(move || {
        let mut buffer = vec![0u8; size];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let chunk = Bytes::copy_from_slice(buffer.get(..read).unwrap_or_default());
                    // A closed receiver means the client hung up; stop reading rather
                    // than pulling the rest of the file into a channel nobody drains.
                    if sender.blocking_send(Ok(chunk)).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    let _ = sender.blocking_send(Err(err));
                    break;
                }
            }
        }
    });

    ReceiverStream::new(receiver)
}
