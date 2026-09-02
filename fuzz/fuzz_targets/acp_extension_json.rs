#![no_main]

use axiom_acp_extension::{
    DeleteConfirmRequest, EventNotification, GetThreadTimelineRequest,
    SetProfilePreferencesRequest, UseApiKeyRequest,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let _ = serde_json::from_slice::<EventNotification>(input);
    let _ = serde_json::from_slice::<SetProfilePreferencesRequest>(input);
    let _ = serde_json::from_slice::<GetThreadTimelineRequest>(input);
    let _ = serde_json::from_slice::<DeleteConfirmRequest>(input);
    if let Ok(request) = serde_json::from_slice::<UseApiKeyRequest>(input) {
        let rendered = format!("{request:?}");
        if !request.api_key.is_empty() {
            assert!(!rendered.contains(&request.api_key));
        }
    }
});
