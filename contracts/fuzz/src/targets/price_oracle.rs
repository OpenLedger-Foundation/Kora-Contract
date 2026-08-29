use arbitrary::Arbitrary;

use crate::gen;
use crate::harness::Protocol;

#[derive(Arbitrary, Debug)]
enum Op {
    Initialize { admin: u8 },
    SetPrice { admin: u8, base: u8, quote: u8, price: i128, ptag: u8 },
    GetPrice { base: u8, quote: u8 },
    Convert { amount: i128, atag: u8, from: u8, to: u8 },
}

pub fn run(data: &[u8]) {
    let p = Protocol::deploy();
    super::drive::<Op, _>(data, |op| {
        let o = &p.price_oracle;
        let env = &p.env;
        match op {
            Op::Initialize { admin } => {
                let _ = o.try_initialize(&p.actor(admin));
            }
            Op::SetPrice { admin, base, quote, price, ptag } => {
                let _ = o.try_set_price(
                    &p.actor(admin),
                    &gen::symbol(env, base),
                    &gen::symbol(env, quote),
                    &gen::amount(price, ptag),
                );
            }
            Op::GetPrice { base, quote } => {
                let _ = o.try_get_price(&gen::symbol(env, base), &gen::symbol(env, quote));
            }
            Op::Convert { amount, atag, from, to } => {
                let _ = o.try_convert(
                    &gen::amount(amount, atag),
                    &gen::symbol(env, from),
                    &gen::symbol(env, to),
                );
            }
        }
    });
}
