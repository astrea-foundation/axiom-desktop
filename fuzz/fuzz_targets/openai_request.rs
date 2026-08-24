#![no_main]

use axiom_openai_compat::{ChatCompletionRequest, CompatMode};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    if let Ok(request) = serde_json::from_slice::<ChatCompletionRequest>(input) {
        let mode = if input.first().is_some_and(|byte| byte % 2 == 0) {
            CompatMode::Lenient
        } else {
            CompatMode::Strict
        };
        let _ = request.into_domain(mode);
    }
});
