use arbitrary::Arbitrary;
use soroban_sdk::{Address, Vec as SVec};

use crate::gen;
use crate::harness::Protocol;

#[derive(Arbitrary, Debug)]
struct ListArgs {
    seller: u8,
    id: u64,
    itag: u8,
    asking: i128,
    atag: u8,
    face: i128,
    ftag: u8,
    token: u8,
    deadline: u64,
    dtag: u8,
    referrer: Option<u8>,
}

#[derive(Arbitrary, Debug)]
enum Op {
    Initialize { admin: u8, nft: u8, pool: u8, treasury: u8, ac: u8, rr: u8, fee: u32, ftag: u8 },
    SetMinFundingBuffer { admin: u8, secs: u64, stag: u8 },
    SetReferrerSplitBps { admin: u8, bps: u32, btag: u8 },
    SetFeeBps { admin: u8, bps: u32, btag: u8, update: bool },
    SetMinDiscountBps { admin: u8, bps: u32, btag: u8 },
    SetTierFeeBps { admin: u8, tier: u8, bps: u32, btag: u8 },
    WhitelistToken { admin: u8, token: u8, remove: bool },
    ListInvoice(ListArgs),
    ListInvoiceWithDecay { base: ListArgs, min_price: i128, mtag: u8, start_ts: u64, stag: u8, end_ts: u64, etag: u8 },
    ListInvoiceWithBidding { base: ListArgs, bidding_deadline: u64, btag: u8 },
    FundInvoice { investor: u8, id: u64, itag: u8, amount: i128, atag: u8 },
    CancelListing { caller: u8, id: u64, itag: u8 },
    RequestCancellation { caller: u8, id: u64, itag: u8 },
    AdminConfirmCancellation { admin: u8, id: u64, itag: u8 },
    ClaimRefund { investor: u8, id: u64, itag: u8 },
    SubmitBid { investor: u8, id: u64, itag: u8, bid_price: i128, ptag: u8, amount: i128, atag: u8 },
    AcceptBids { caller: u8, id: u64, itag: u8, investors: [u8; 3], count: u8 },
    Reads { id: u64, itag: u8, investor: u8, tier: u8, token: u8 },
    ProposeUpgrade { admin: u8, hash: u8 },
    ExecuteUpgrade { admin: u8 },
}

fn referrer(p: &Protocol, r: Option<u8>) -> Option<Address> {
    return r.map(|sel| p.actor(sel));
}

pub fn run(data: &[u8]) {
    let p = Protocol::deploy();
    super::drive::<Op, _>(data, |op| {
        let c = &p.marketplace;
        let env = &p.env;
        match op {
            Op::Initialize { admin, nft, pool, treasury, ac, rr, fee, ftag } => {
                let _ = c.try_initialize(
                    &p.actor(admin),
                    &p.actor(nft),
                    &p.actor(pool),
                    &p.actor(treasury),
                    &p.actor(ac),
                    &p.actor(rr),
                    &gen::small_u32(fee, ftag),
                );
            }
            Op::SetMinFundingBuffer { admin, secs, stag } => {
                let _ = c.try_set_min_funding_buffer(&p.actor(admin), &gen::ts_or_id(secs, stag));
            }
            Op::SetReferrerSplitBps { admin, bps, btag } => {
                let _ = c.try_set_referrer_split_bps(&p.actor(admin), &gen::small_u32(bps, btag));
            }
            Op::SetFeeBps { admin, bps, btag, update } => {
                let v = gen::small_u32(bps, btag);
                if update {
                    let _ = c.try_update_fee_bps(&p.actor(admin), &v);
                } else {
                    let _ = c.try_set_fee_bps(&p.actor(admin), &v);
                }
            }
            Op::SetMinDiscountBps { admin, bps, btag } => {
                let _ = c.try_set_min_discount_bps(&p.actor(admin), &gen::small_u32(bps, btag));
            }
            Op::SetTierFeeBps { admin, tier, bps, btag } => {
                let _ = c.try_set_tier_fee_bps(&p.actor(admin), &gen::risk_tier(tier), &gen::small_u32(bps, btag));
            }
            Op::WhitelistToken { admin, token, remove } => {
                if remove {
                    let _ = c.try_remove_token_whitelist(&p.actor(admin), &p.actor(token));
                } else {
                    let _ = c.try_whitelist_token(&p.actor(admin), &p.actor(token));
                }
            }
            Op::ListInvoice(a) => {
                let _ = c.try_list_invoice(
                    &p.actor(a.seller),
                    &gen::ts_or_id(a.id, a.itag),
                    &gen::amount(a.asking, a.atag),
                    &gen::amount(a.face, a.ftag),
                    &p.actor(a.token),
                    &gen::ts_or_id(a.deadline, a.dtag),
                    &referrer(&p, a.referrer),
                );
            }
            Op::ListInvoiceWithDecay { base, min_price, mtag, start_ts, stag, end_ts, etag } => {
                let _ = c.try_list_invoice_with_decay(
                    &p.actor(base.seller),
                    &gen::ts_or_id(base.id, base.itag),
                    &gen::amount(base.asking, base.atag),
                    &gen::amount(base.face, base.ftag),
                    &p.actor(base.token),
                    &gen::ts_or_id(base.deadline, base.dtag),
                    &referrer(&p, base.referrer),
                    &gen::amount(min_price, mtag),
                    &gen::ts_or_id(start_ts, stag),
                    &gen::ts_or_id(end_ts, etag),
                );
            }
            Op::ListInvoiceWithBidding { base, bidding_deadline, btag } => {
                let _ = c.try_list_invoice_with_bidding(
                    &p.actor(base.seller),
                    &gen::ts_or_id(base.id, base.itag),
                    &gen::amount(base.asking, base.atag),
                    &gen::amount(base.face, base.ftag),
                    &p.actor(base.token),
                    &gen::ts_or_id(base.deadline, base.dtag),
                    &referrer(&p, base.referrer),
                    &gen::ts_or_id(bidding_deadline, btag),
                );
            }
            Op::FundInvoice { investor, id, itag, amount, atag } => {
                let _ = c.try_fund_invoice(
                    &p.actor(investor),
                    &gen::ts_or_id(id, itag),
                    &gen::amount(amount, atag),
                );
            }
            Op::CancelListing { caller, id, itag } => {
                let _ = c.try_cancel_listing(&p.actor(caller), &gen::ts_or_id(id, itag));
            }
            Op::RequestCancellation { caller, id, itag } => {
                let _ = c.try_request_cancellation(&p.actor(caller), &gen::ts_or_id(id, itag));
            }
            Op::AdminConfirmCancellation { admin, id, itag } => {
                let _ = c.try_admin_confirm_cancellation(&p.actor(admin), &gen::ts_or_id(id, itag));
            }
            Op::ClaimRefund { investor, id, itag } => {
                let _ = c.try_claim_refund(&p.actor(investor), &gen::ts_or_id(id, itag));
            }
            Op::SubmitBid { investor, id, itag, bid_price, ptag, amount, atag } => {
                let _ = c.try_submit_bid(
                    &p.actor(investor),
                    &gen::ts_or_id(id, itag),
                    &gen::amount(bid_price, ptag),
                    &gen::amount(amount, atag),
                );
            }
            Op::AcceptBids { caller, id, itag, investors, count } => {
                let mut v: SVec<Address> = SVec::new(env);
                for i in 0..(count as usize % 4) {
                    v.push_back(p.actor(investors[i % investors.len()]));
                }
                let _ = c.try_accept_bids(&p.actor(caller), &gen::ts_or_id(id, itag), &v);
            }
            Op::Reads { id, itag, investor, tier, token } => {
                let i = gen::ts_or_id(id, itag);
                let _ = c.try_get_listing(&i);
                let _ = c.try_get_current_price(&i);
                let _ = c.try_get_decay_schedule(&i);
                let _ = c.try_get_bid(&i, &p.actor(investor));
                let _ = c.try_get_config();
                let _ = c.try_get_admin();
                let _ = c.try_get_fee_bps();
                let _ = c.try_get_min_discount_bps();
                let _ = c.try_get_min_funding_buffer();
                let _ = c.try_get_tier_fee_bps(&gen::risk_tier(tier));
                let _ = c.try_is_token_whitelisted(&p.actor(token));
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
