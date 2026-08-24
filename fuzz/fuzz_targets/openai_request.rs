#![no_main]

use axiom_openai_compat::ChatCompletionRequest;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    if let Ok(request) = serde_json::from_slice::<ChatCompletionRequest>(input) {
        let _ = request.into_domain();
    }
});
