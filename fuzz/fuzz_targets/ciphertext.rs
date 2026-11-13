#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    axiom_secure_client::fuzz_support::ciphertext(input);
});
