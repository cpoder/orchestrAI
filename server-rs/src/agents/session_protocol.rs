//! Wire protocol for the mini-supervisor ↔ client session.
//!
//! Frames are length-prefixed (4-byte big-endian u32) followed by a
//! postcard-encoded [`Message`] payload. Frames cap at `MAX_FRAME_BYTES` so a
//! buggy or malicious peer can't force an unbounded allocation.
#![allow(dead_code)] // wired up by later supervisor tasks

use serde::{Deserialize, Serialize};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Hard cap on a single frame's payload. 8 MiB is plenty for a screenful of
/// terminal output and keeps a misbehaving peer from asking us to allocate
/// gigabytes.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    /// Stdin bytes from client → supervisor (forwarded to the PTY).
    Input(Vec<u8>),
    /// Stdout/stderr bytes from supervisor → client (PTY master output).
    Output(Vec<u8>),
    /// Window-size change from client → supervisor.
    Resize { cols: u16, rows: u16 },
    /// Client asks the supervisor to terminate the session.
    Kill,
    /// Keepalive request.
    Ping,
    /// Keepalive response.
    Pong,
}

/// Encode a message to its wire bytes (payload only, no length prefix).
pub fn encode(msg: &Message) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(msg)
}

/// Decode a message from its wire bytes (payload only, no length prefix).
pub fn decode(bytes: &[u8]) -> Result<Message, postcard::Error> {
    postcard::from_bytes(bytes)
}

/// Write a single length-prefixed frame to an async writer.
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, msg: &Message) -> io::Result<()> {
    let payload = encode(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "frame payload {} exceeds cap {}",
                payload.len(),
                MAX_FRAME_BYTES
            ),
        ));
    }
    let len = (payload.len() as u32).to_be_bytes();
    writer.write_all(&len).await?;
    writer.write_all(&payload).await?;
    Ok(())
}

/// Read a single length-prefixed frame from an async reader.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Message> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {len} exceeds cap {MAX_FRAME_BYTES}"),
        ));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    decode(&payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    fn round_trip(msg: &Message) {
        let bytes = encode(msg).expect("encode");
        let decoded = decode(&bytes).expect("decode");
        assert_eq!(*msg, decoded);
    }

    #[test]
    fn round_trip_input() {
        round_trip(&Message::Input(b"hello\x00\xff world".to_vec()));
        round_trip(&Message::Input(Vec::new()));
    }

    #[test]
    fn round_trip_output() {
        round_trip(&Message::Output(vec![0, 1, 2, 3, 255]));
    }

    #[test]
    fn round_trip_resize() {
        round_trip(&Message::Resize { cols: 80, rows: 24 });
        round_trip(&Message::Resize { cols: 0, rows: 0 });
        round_trip(&Message::Resize {
            cols: u16::MAX,
            rows: u16::MAX,
        });
    }

    #[test]
    fn round_trip_control() {
        round_trip(&Message::Kill);
        round_trip(&Message::Ping);
        round_trip(&Message::Pong);
    }

    #[tokio::test]
    async fn framed_round_trip_all_variants() {
        let (mut a, mut b) = duplex(64 * 1024);
        let messages = vec![
            Message::Input(b"type this".to_vec()),
            Message::Output(b"printed output\n".to_vec()),
            Message::Resize {
                cols: 120,
                rows: 40,
            },
            Message::Kill,
            Message::Ping,
            Message::Pong,
            Message::Input(Vec::new()),
            Message::Output(vec![0xAA; 16 * 1024]),
        ];

        let send = messages.clone();
        let writer = tokio::spawn(async move {
            for m in &send {
                write_frame(&mut a, m).await.unwrap();
            }
        });

        for expected in &messages {
            let got = read_frame(&mut b).await.unwrap();
            assert_eq!(*expected, got);
        }
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn oversized_declared_length_rejected() {
        let (mut a, mut b) = duplex(64);
        let bogus = ((MAX_FRAME_BYTES as u32) + 1).to_be_bytes();
        a.write_all(&bogus).await.unwrap();
        drop(a);
        let err = read_frame(&mut b).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn decode_rejects_garbage() {
        // An empty byte slice can't encode any variant tag.
        assert!(decode(&[]).is_err());
    }
}
