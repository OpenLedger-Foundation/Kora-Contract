use arbitrary::Arbitrary;
use soroban_sdk::Vec as SVec;

use crate::gen;
use crate::harness::Protocol;
use kora_shared::types::{Installment, InstallmentSchedule};

#[derive(Arbitrary, Debug)]
enum Op {
    Initialize { admin: u8, nft: u8, rr: u8, treasury: u8, ac: u8, penalty: u32, ptag: u8, oracle: u8, max_bps: u32, mtag: u8 },
    ReleaseFunds { marketplace: u8, id: u64, itag: u8, token: u8 },
    SetMaxPositionBps { admin: u8, bps: u32, btag: u8 },
    RecordPosition { caller: u8, id: u64, itag: u8, investor: u8, contributed: i128, ctag: u8, total: i128, ttag: u8 },
    Repay { payer: u8, id: u64, itag: u8, token: u8, amount: i128, atag: u8 },
    MarkDefault { admin: u8, id: u64, itag: u8, token: u8 },
    ProposeEarlySettlement { sme: u8, id: u64, itag: u8, amount: i128, atag: u8 },
    AcceptEarlySettlement { investor: u8, id: u64, itag: u8 },
    CancelEarlySettlement { sme: u8, id: u64, itag: u8 },
    SetInstallmentSchedule { admin: u8, id: u64, itag: u8, a_amt: i128, a_tag: u8, a_due: u64, a_dtag: u8, count: u8, next_index: u32 },
    ListPositionForSale { seller: u8, id: u64, itag: u8, token: u8, price: i128, ptag: u8 },
    BuyPosition { buyer: u8, id: u64, itag: u8, seller: u8 },
    Reads { id: u64, itag: u8, offset: u32, limit: u32, ltag: u8 },
    ProposeUpgrade { admin: u8, hash: u8 },
    ExecuteUpgrade { admin: u8 },
}

pub fn run(data: &[u8]) {
    let p = Protocol::deploy();
    super::drive::<Op, _>(data, |op| {
        let c = &p.pool;
        let env = &p.env;
        match op {
            Op::Initialize { admin, nft, rr, treasury, ac, penalty, ptag, oracle, max_bps, mtag } => {
                let _ = c.try_initialize(
                    &p.actor(admin),
                    &p.actor(nft),
                    &p.actor(rr),
                    &p.actor(treasury),
                    &p.actor(ac),
                    &gen::small_u32(penalty, ptag),
                    &p.actor(oracle),
                    &gen::small_u32(max_bps, mtag),
                );
            }
            Op::ReleaseFunds { marketplace, id, itag, token } => {
                let _ = c.try_release_funds(&p.actor(marketplace), &gen::ts_or_id(id, itag), &p.actor(token));
            }
            Op::SetMaxPositionBps { admin, bps, btag } => {
                let _ = c.try_set_max_position_bps(&p.actor(admin), &gen::small_u32(bps, btag));
            }
            Op::RecordPosition { caller, id, itag, investor, contributed, ctag, total, ttag } => {
                let _ = c.try_record_position(
                    &p.actor(caller),
                    &gen::ts_or_id(id, itag),
                    &p.actor(investor),
                    &gen::amount(contributed, ctag),
                    &gen::amount(total, ttag),
                );
            }
            Op::Repay { payer, id, itag, token, amount, atag } => {
                let _ = c.try_repay(
                    &p.actor(payer),
                    &gen::ts_or_id(id, itag),
                    &p.actor(token),
                    &gen::amount(amount, atag),
                );
            }
            Op::MarkDefault { admin, id, itag, token } => {
                let _ = c.try_mark_default(&p.actor(admin), &gen::ts_or_id(id, itag), &p.actor(token));
            }
            Op::ProposeEarlySettlement { sme, id, itag, amount, atag } => {
                let _ = c.try_propose_early_settlement(
                    &p.actor(sme),
                    &gen::ts_or_id(id, itag),
                    &gen::amount(amount, atag),
                );
            }
            Op::AcceptEarlySettlement { investor, id, itag } => {
                let _ = c.try_accept_early_settlement(&p.actor(investor), &gen::ts_or_id(id, itag));
            }
            Op::CancelEarlySettlement { sme, id, itag } => {
                let _ = c.try_cancel_early_settlement(&p.actor(sme), &gen::ts_or_id(id, itag));
            }
            Op::SetInstallmentSchedule { admin, id, itag, a_amt, a_tag, a_due, a_dtag, count, next_index } => {
                let mut items: SVec<Installment> = SVec::new(env);
                for _ in 0..(count % 6) {
                    items.push_back(Installment {
                        amount: gen::amount(a_amt, a_tag),
                        due_date: gen::ts_or_id(a_due, a_dtag),
                        paid: false,
                    });
                }
                let schedule = InstallmentSchedule { installments: items, next_index };
                let _ = c.try_set_installment_schedule(&p.actor(admin), &gen::ts_or_id(id, itag), &schedule);
            }
            Op::ListPositionForSale { seller, id, itag, token, price, ptag } => {
                let _ = c.try_list_position_for_sale(
                    &p.actor(seller),
                    &gen::ts_or_id(id, itag),
                    &p.actor(token),
                    &gen::amount(price, ptag),
                );
            }
            Op::BuyPosition { buyer, id, itag, seller } => {
                let _ = c.try_buy_position(&p.actor(buyer), &gen::ts_or_id(id, itag), &p.actor(seller));
            }
            Op::Reads { id, itag, offset, limit, ltag } => {
                let i = gen::ts_or_id(id, itag);
                let _ = c.try_get_early_settlement(&i);
                let _ = c.try_get_pool(&i);
                let _ = c.try_get_positions(&i);
                let _ = c.try_get_protocol_stats();
                let _ = c.try_get_installment_schedule(&i);
                let _ = c.try_get_max_position_bps();
                let _ = c.try_get_positions_count(&i);
                let _ = c.try_get_positions_page(
                    &i,
                    &gen::small_u32(offset, ltag),
                    &gen::small_u32(limit, ltag),
                );
            }
            Op::ProposeUpgrade { admin, hash } => {
                let _ = c.try_propose_upgrade(&p.actor(admin), &gen::hash32(env, hash));
            }
            Op::ExecuteUpgrade { admin } => {
                let _ = c.try_execute_upgrade(&p.actor(admin));
            }
        }
    });
}
