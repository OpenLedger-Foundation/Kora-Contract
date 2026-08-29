//! One module per contract. Each exposes `run(&[u8])`, which decodes the input
//! as a sequence of `Op`s and applies them to a fresh [`crate::harness::Protocol`]
//! via the generated `try_*` client methods.
//!
//! `try_*` is deliberate: a returned `Err` is a contract correctly rejecting bad
//! input and is ignored. Only a panic (host trap surfaced as unwind, arithmetic
//! overflow under debug assertions, an `unwrap` on absent state) escapes `run`
//! and is reported as a finding.

pub mod access_control;
pub mod financing_pool;
pub mod invoice_nft;
pub mod marketplace;
pub mod price_oracle;
pub mod risk_registry;
pub mod treasury;

/// Decode up to [`crate::MAX_OPS`] operations from `data` and apply each with
/// `f`. Stops when the buffer is exhausted or an `Op` fails to decode.
pub(crate) fn drive<Op, F>(data: &[u8], mut f: F)
where
    Op: for<'a> arbitrary::Arbitrary<'a>,
    F: FnMut(Op),
{
    let mut u = arbitrary::Unstructured::new(data);
    for _ in 0..crate::MAX_OPS {
        if u.is_empty() {
            return;
        }
        match Op::arbitrary(&mut u) {
            Ok(op) => f(op),
            Err(_) => return,
        }
    }
}
