#![no_main]

use axiom_acp_extension::{
    BillingInvoiceCreateRequest, DeleteConfirmRequest, EventNotification, GetThreadTimelineRequest,
    NativeLoginStartRequest, SetProfilePreferencesRequest,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let _ = serde_json::from_slice::<EventNotification>(input);
    let _ = serde_json::from_slice::<SetProfilePreferencesRequest>(input);
    let _ = serde_json::from_slice::<GetThreadTimelineRequest>(input);
    let _ = serde_json::from_slice::<DeleteConfirmRequest>(input);
    let _ = serde_json::from_slice::<NativeLoginStartRequest>(input);
    let _ = serde_json::from_slice::<BillingInvoiceCreateRequest>(input);
});
