#![no_main]

use axiom_inference::{ToolCallAccumulator, ToolCallDelta};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let mut calls = ToolCallAccumulator::default();
    for (position, chunk) in input.chunks(5).enumerate() {
        let index = chunk.first().copied().map_or(0, usize::from);
        let field = chunk.get(1).copied().unwrap_or_default() % 4;
        let value = String::from_utf8_lossy(chunk.get(2..).unwrap_or_default()).into_owned();
        let mut delta = ToolCallDelta {
            index,
            ..ToolCallDelta::default()
        };
        match field {
            0 => delta.id = Some(value),
            1 => delta.kind = Some(value),
            2 => delta.name = Some(value),
            _ => delta.arguments = Some(value),
        }
        let _ = calls.push(delta);
        if position > 512 {
            break;
        }
    }
    let _ = calls.finish();
});
