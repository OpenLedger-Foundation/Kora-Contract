#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    kora_fuzz::targets::treasury::run(data);
});
