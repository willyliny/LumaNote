//! Fixture-driven adapter tests over the replay transport (no network).

mod anthropic;
mod openai_chat;

use crate::ports::fakes::{Chunking, LineEnding, ReplayResponse};

/// Chunk sizes every fixture is replayed with: single bytes (splits every UTF-8
/// character and SSE line), small primes, and whole-body delivery.
pub(super) const CHUNKS: &[usize] = &[1, 2, 3, 7, 64, 0];

pub(super) fn chunked(r: ReplayResponse, size: usize) -> ReplayResponse {
    if size == 0 {
        r.with_chunking(Chunking::Whole)
    } else {
        r.chunked(size)
    }
}

pub(super) fn variants(r: &ReplayResponse) -> Vec<(String, ReplayResponse)> {
    let mut out = Vec::new();
    for &c in CHUNKS {
        out.push((format!("lf/{c}"), chunked(r.clone(), c)));
        out.push((
            format!("crlf/{c}"),
            chunked(r.clone().with_line_ending(LineEnding::Crlf), c),
        ));
    }
    out
}
