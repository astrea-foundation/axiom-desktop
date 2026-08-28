//! Narrow adapters that let cargo-fuzz reach otherwise-private wire parsers.

use crate::{
    providers::near::{attestation, crypto::ClientIdentity, inference},
    relay::sse::SseDecoder,
};

/// Exercise authenticated-ciphertext decoding with arbitrary wire bytes.
pub fn ciphertext(input: &[u8]) {
    let identity = ClientIdentity::generate();
    let _ = identity.decrypt_hex(&hex::encode(input));
    if let Ok(text) = std::str::from_utf8(input) {
        let _ = identity.decrypt_hex(text);
    }
}

/// Exercise the provider evidence JSON and claim parsers without networking.
pub fn evidence(input: &[u8]) {
    attestation::fuzz_parse_report(input);
}

/// Exercise the closed NEAR signed-receipt parser and all bounded decoders.
pub fn near_receipt(input: &[u8]) {
    inference::fuzz_parse_receipt(input);
}

/// Exercise SSE framing with several deterministic transport fragmentations.
pub fn sse(input: &[u8]) {
    for chunk_size in [1, 2, 7, 64, 1024] {
        let mut decoder = SseDecoder::new(64 * 1024, 256 * 1024);
        let mut failed = false;
        for chunk in input.chunks(chunk_size) {
            if decoder.feed(chunk).is_err() {
                failed = true;
                break;
            }
        }
        if !failed {
            let _ = decoder.finish();
        }
    }
}
