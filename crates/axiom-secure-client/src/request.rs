//! Bound serialized provider/relay bytes before allocating an oversized body.
use std::io::{self, Write};

use axiom_inference::ProviderFailureKind;
use serde::Serialize;

use crate::{Result, SecureClientError};

pub(crate) const REQUEST_TOO_LARGE: &str = "This encrypted request exceeds the provider upload limit. Reduce its attachments or start a new conversation and try again.";

pub(crate) fn serialize_bounded(value: &impl Serialize, limit: usize) -> Result<Vec<u8>> {
    struct BoundedWriter {
        bytes: Vec<u8>,
        limit: usize,
        exceeded: bool,
    }
    impl Write for BoundedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                self.exceeded = true;
                return Err(io::Error::other("request byte limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        limit,
        exceeded: false,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| {
        SecureClientError::new(
            ProviderFailureKind::InvalidRequest,
            if writer.exceeded {
                REQUEST_TOO_LARGE
            } else {
                "Request could not be serialized."
            },
        )
    })?;
    Ok(writer.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_limit_counts_actual_json_escaping_and_utf8_at_the_boundary() {
        let value = serde_json::json!({"text": "café 😀\n\"\\", "ciphertext": "ab".repeat(72)});
        let expected = serde_json::to_vec(&value).unwrap();
        assert_eq!(serialize_bounded(&value, expected.len()).unwrap(), expected);
        for limit in [0, expected.len() - 1] {
            let error = serialize_bounded(&value, limit).unwrap_err();
            assert_eq!(error.kind(), ProviderFailureKind::InvalidRequest);
            assert_eq!(error.safe_detail(), REQUEST_TOO_LARGE);
            assert!(!error.retryable());
        }
    }
}
