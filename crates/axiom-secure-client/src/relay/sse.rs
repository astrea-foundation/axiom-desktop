use crate::{
    Result, SecureClientError,
    relay::dto::{
        CancelledPayload, CompletedPayload, CreatedPayload, DeltaPayload, FailedPayload, RunPayload,
    },
};

#[derive(Debug)]
pub(crate) enum RelayEvent {
    RunCreated(CreatedPayload),
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
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Result<Vec<RelayEvent>> {
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| invalid("relay stream exceeds the configured limit"))?;
        if self.total_bytes > self.stream_limit {
            return Err(invalid("relay stream exceeds the configured limit"));
        }

        let mut events = Vec::new();
        for &byte in bytes {
            self.line.push(byte);
            if self.event_bytes.saturating_add(self.line.len()) > self.event_limit {
                return Err(invalid("relay SSE event exceeds the configured limit"));
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
                .ok_or_else(|| invalid("relay SSE event name is missing"))?;
            if self.data.is_empty() {
                return Err(invalid("relay SSE event data is missing"));
            }
            let data = self.data.join("\n");
            self.data.clear();
            self.event_bytes = 0;
            events.push(parse_event(&name, &data)?);
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
            _ => return Err(invalid("relay SSE field is not supported")),
        }
        Ok(())
    }
}

fn parse_event(name: &str, data: &str) -> Result<RelayEvent> {
    macro_rules! decode {
        ($payload:ty, $variant:ident) => {
            serde_json::from_str::<$payload>(data)
                .map(RelayEvent::$variant)
                .map_err(|_| invalid("relay SSE event schema is invalid"))
        };
    }
    match name {
        "run.created" => decode!(CreatedPayload, RunCreated),
        "message.encrypted_delta" => decode!(DeltaPayload, Delta),
        "message.encrypted_completed" => decode!(CompletedPayload, MessageCompleted),
        "run.completed" => decode!(RunPayload, RunCompleted),
        "run.cancelled" => decode!(CancelledPayload, Cancelled),
        "run.failed" => decode!(FailedPayload, Failed),
        _ => Err(invalid("relay SSE event name is not supported")),
    }
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
    fn rejects_unknown_events_invalid_utf8_oversize_and_abrupt_eof() {
        let mut unknown = SseDecoder::new(1024, 4096);
        assert!(unknown.feed(b"event: surprise\ndata: {}\n\n").is_err());

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
