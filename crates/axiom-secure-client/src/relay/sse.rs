use crate::{
    Result, SecureClientError,
    relay::dto::{
        CancelledPayload, CompletedPayload, CreatedPayload, DeltaPayload, FailedPayload, RunPayload,
    },
};

#[derive(Debug)]
pub(crate) enum RelayEvent {
    RunCreated(CreatedPayload),
    Accounting(axiom_inference::RequestUsage),
    Finalizing(RunPayload),
    Delta(DeltaPayload),
    MessageCompleted(CompletedPayload),
    RunCompleted(RunPayload),
    Cancelled(CancelledPayload),
    Failed(FailedPayload),
}

pub(crate) struct SseDecoder {
    event_limit: usize,
    stream_limit: usize,
    total_bytes: usize,
    event_bytes: usize,
    line: Vec<u8>,
    event_name: Option<String>,
    data: Vec<String>,
    proof_run_id: Option<String>,
    proof_parts: u64,
    proof_body: String,
}

impl SseDecoder {
    pub(crate) fn new(event_limit: usize, stream_limit: usize) -> Self {
        Self {
            event_limit,
            stream_limit,
            total_bytes: 0,
            event_bytes: 0,
            line: Vec::new(),
            event_name: None,
            data: Vec::new(),
            proof_run_id: None,
            proof_parts: 0,
            proof_body: String::new(),
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Result<Vec<RelayEvent>> {
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes.len())
            .ok_or_else(too_large)?;
        if self.total_bytes > self.stream_limit {
            return Err(too_large());
        }

        let mut events = Vec::new();
        for &byte in bytes {
            self.line.push(byte);
            if self.event_bytes.saturating_add(self.line.len()) > self.event_limit {
                return Err(too_large());
            }
            if byte == b'\n' {
                let mut line = std::mem::take(&mut self.line);
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.event_bytes = self.event_bytes.saturating_add(line.len() + 1);
                self.accept_line(&line, &mut events)?;
            }
        }
        Ok(events)
    }

    pub(crate) fn finish(self) -> Result<()> {
        if self.line.is_empty()
            && self.event_name.is_none()
            && self.data.is_empty()
            && self.event_bytes == 0
            && self.proof_parts == 0
        {
            Ok(())
        } else {
            Err(invalid("relay SSE stream ended inside an event"))
        }
    }

    fn accept_line(&mut self, line: &[u8], events: &mut Vec<RelayEvent>) -> Result<()> {
        let line = std::str::from_utf8(line)
            .map_err(|_| invalid("relay SSE control data is not UTF-8"))?;
        if line.is_empty() {
            if self.event_name.is_none() && self.data.is_empty() {
                self.event_bytes = 0;
                return Ok(());
            }
            let name = self
                .event_name
                .take()
                .unwrap_or_else(|| "message".to_owned());
            if self.data.is_empty() {
                return Err(invalid("relay SSE event data is missing"));
            }
            let data = self.data.join("\n");
            self.data.clear();
            self.event_bytes = 0;
            if name == "message.proof_chunk" {
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Part {
                    run_id: String,
                    index: u64,
                    data: String,
                }
                let part: Part =
                    serde_json::from_str(&data).map_err(|_| invalid("invalid receipt chunk"))?;
                if part.run_id.is_empty()
                    || part.run_id.len() > 256
                    || part.index != self.proof_parts
                    || part.data.is_empty()
                    || part.data.len() > 256 * 1024
                    || self.proof_body.len().saturating_add(part.data.len())
                        > (32 * 1024 * 1024 / 3 + 1) * 4
                    || self
                        .proof_run_id
                        .as_ref()
                        .is_some_and(|id| id != &part.run_id)
                {
                    return Err(invalid("receipt chunk sequence or size is invalid"));
                }
                self.proof_run_id = Some(part.run_id);
                self.proof_parts += 1;
                self.proof_body.push_str(&part.data);
            } else if let Some(mut event) = parse_event(&name, &data)? {
                if let RelayEvent::MessageCompleted(payload) = &mut event {
                    if let Some(proof) = payload
                        .proof
                        .as_mut()
                        .and_then(serde_json::Value::as_object_mut)
                    {
                        let count = proof.remove("response_body_chunk_count");
                        if self.proof_parts > 0 || count.is_some() {
                            if count.and_then(|v| v.as_u64()) != Some(self.proof_parts)
                                || self.proof_parts == 0
                                || self.proof_run_id.as_deref() != Some(&payload.run_id)
                                || proof.contains_key("response_body_base64")
                            {
                                return Err(invalid("incomplete or duplicated receipt chunks"));
                            }
                            proof.insert(
                                "response_body_base64".into(),
                                serde_json::Value::String(std::mem::take(&mut self.proof_body)),
                            );
                            self.proof_parts = 0;
                            self.proof_run_id = None;
                        }
                    } else if self.proof_parts > 0 {
                        return Err(invalid("receipt chunks have no terminal proof"));
                    }
                }
                events.push(event);
            }
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" if self.event_name.is_none() => self.event_name = Some(value.to_owned()),
            "data" => self.data.push(value.to_owned()),
            "event" => return Err(invalid("relay SSE event name is duplicated")),
            // `id`, `retry`, and future SSE fields are transport metadata.
            // They remain covered by the event and stream byte limits and do
            // not alter Axiom's own run IDs or sequence validation.
            _ => {}
        }
        Ok(())
    }
}

fn parse_event(name: &str, data: &str) -> Result<Option<RelayEvent>> {
    macro_rules! decode {
        ($payload:ty, $variant:ident) => {
            serde_json::from_str::<$payload>(data)
                .map(RelayEvent::$variant)
                .map(Some)
                .map_err(|_| invalid("relay SSE event schema is invalid"))
        };
    }
    match name {
        "run.created" => decode!(CreatedPayload, RunCreated),
        "run.accounting" => decode!(axiom_inference::RequestUsage, Accounting),
        "message.encrypted_delta" => decode!(DeltaPayload, Delta),
        "message.encrypted_completed" => decode!(CompletedPayload, MessageCompleted),
        "run.completed" => decode!(RunPayload, RunCompleted),
        "run.finalizing" => decode!(RunPayload, Finalizing),
        "run.cancelled" => decode!(CancelledPayload, Cancelled),
        "run.failed" => decode!(FailedPayload, Failed),
        // Additive event types are skipped. A server cannot use this to omit
        // required known terminal events: the relay state machine still
        // rejects an incomplete stream at EOF.
        _ => Ok(None),
    }
}

fn too_large() -> SecureClientError {
    SecureClientError::with_code(
        axiom_inference::ProviderFailureKind::InvalidResponse,
        crate::SecureErrorCode::ResponseTooLarge,
        "Response exceeded the allowed size.",
        false,
    )
}

fn invalid(detail: &'static str) -> SecureClientError {
    SecureClientError::new(
        axiom_inference::ProviderFailureKind::InvalidResponse,
        detail,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_chunks_are_reassembled_only_at_an_exact_terminal_count() {
        let part = "event: message.proof_chunk\ndata: {\"run_id\":\"r1\",\"index\":0,\"data\":\"e30=\"}\n\n";
        let end = "event: message.encrypted_completed\ndata: {\"run_id\":\"r1\",\"finish_reason\":\"stop\",\"usage\":{},\"proof\":{\"response_body_chunk_count\":1}}\n\n";
        let mut decoder = SseDecoder::new(1024, 4096);
        assert!(decoder.feed(part.as_bytes()).unwrap().is_empty());
        let events = decoder.feed(end.as_bytes()).unwrap();
        let RelayEvent::MessageCompleted(payload) = &events[0] else {
            panic!("completion")
        };
        assert_eq!(
            payload.proof.as_ref().unwrap()["response_body_base64"],
            "e30="
        );
        decoder.finish().unwrap();
        let mut duplicate = SseDecoder::new(1024, 4096);
        duplicate.feed(part.as_bytes()).unwrap();
        assert!(duplicate.feed(part.as_bytes()).is_err());
        let mut truncated = SseDecoder::new(1024, 4096);
        truncated.feed(part.as_bytes()).unwrap();
        assert!(truncated.finish().is_err());
        let mut missing = SseDecoder::new(1024, 4096);
        assert!(missing.feed(end.as_bytes()).is_err());
    }

    const STREAM: &str = concat!(
        ": heartbeat\r\n\r\n",
        "event: run.created\r\n",
        "data: {\"run_id\":\"r1\",\"inference_encryption\":\"provider_e2ee_v2\"}\r\n\r\n",
        "event: message.encrypted_completed\n",
        "data: {\"run_id\":\"r1\",\"usage\":{},\"finish_reason\":\"stop\"}\n\n",
        "event: run.completed\n",
        "data: {\"run_id\":\"r1\"}\n\n",
    );

    #[test]
    fn parses_every_byte_fragment_with_heartbeats_and_crlf() {
        let mut decoder = SseDecoder::new(8 * 1024, 32 * 1024);
        let mut events = Vec::new();
        for byte in STREAM.as_bytes() {
            events.extend(decoder.feed(std::slice::from_ref(byte)).unwrap());
        }
        decoder.finish().unwrap();
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], RelayEvent::RunCreated(_)));
        assert!(matches!(events[1], RelayEvent::MessageCompleted(_)));
        assert!(matches!(events[2], RelayEvent::RunCompleted(_)));
    }

    #[test]
    fn skips_unknown_events_and_fields_but_rejects_malformed_transport() {
        let mut unknown = SseDecoder::new(1024, 4096);
        assert!(
            unknown
                .feed(b"event: surprise\nid: 42\nfuture: metadata\ndata: {}\n\n")
                .unwrap()
                .is_empty()
        );
        unknown.finish().unwrap();

        let mut additive = SseDecoder::new(1024, 4096);
        let events = additive
            .feed(
                b"event: run.created\nid: 42\ndata: {\"run_id\":\"r1\",\"inference_encryption\":\"provider_e2ee_v2\",\"future\":true}\n\n",
            )
            .unwrap();
        assert!(matches!(events.as_slice(), [RelayEvent::RunCreated(_)]));
        additive.finish().unwrap();

        let mut utf8 = SseDecoder::new(1024, 4096);
        assert!(utf8.feed(b"event: run.created\ndata: \xff\n\n").is_err());

        let mut oversize = SseDecoder::new(8, 4096);
        assert!(oversize.feed(b"event: run.created\n").is_err());

        let mut abrupt = SseDecoder::new(1024, 4096);
        abrupt.feed(b"event: run.created\n").unwrap();
        assert!(abrupt.finish().is_err());
    }

    #[test]
    fn preserves_unicode_across_transport_boundaries() {
        let event = "event: run.failed\ndata: {\"run_id\":\"r1\",\"error\":\"bad 🌸\"}\n\n";
        let blossom = event.find('🌸').unwrap();
        let bytes = event.as_bytes();
        let mut decoder = SseDecoder::new(1024, 4096);
        assert!(decoder.feed(&bytes[..=blossom]).unwrap().is_empty());
        let parsed = decoder.feed(&bytes[blossom + 1..]).unwrap();
        assert!(matches!(parsed.as_slice(), [RelayEvent::Failed(_)]));
        decoder.finish().unwrap();
    }
}
