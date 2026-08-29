use arbitrary::Arbitrary;
use soroban_sdk::{Vec as SVec};

use crate::gen;
use crate::harness::Protocol;
use kora_invoice_nft::BatchInvoiceInput;

#[derive(Arbitrary, Debug)]
struct MintArgs {
    sme: u8,
    hash: u8,
    amount: i128,
    atag: u8,
    currency: u8,
    due_date: u64,
    dtag: u8,
    cid: u8,
    score: u32,
    stag: u8,
    with_notes: bool,
}

#[derive(Arbitrary, Debug)]
enum Op {
    Initialize { admin: u8, ac: u8 },
    SetRiskRegistry { admin: u8, rr: u8 },
    Migrate { admin: u8 },
    SetAuthorizedCallers { admin: u8, mp: u8, pool: u8 },
    MintInvoice(MintArgs),
    MintBatch { sme: u8, a: MintArgs, b: MintArgs, count: u8 },
    AmendInvoice { sme: u8, id: u64, itag: u8, hash: u8, amount: i128, atag: u8, due: u64, dtag: u8, cid: u8, score: u32, stag: u8 },
    WithdrawInvoice { sme: u8, id: u64, itag: u8 },
    Transition { kind: u8, caller: u8, id: u64, itag: u8 },
    CommitMetadataHash { sme: u8, id: u64, itag: u8, hash: u8 },
    Freeze { admin: u8, id: u64, itag: u8, unfreeze: bool },
    FreezeSme { admin: u8, sme: u8, max: u32, mtag: u8, unfreeze: bool },
    RefreshRiskScore { caller: u8, id: u64, itag: u8 },
    Currency { admin: u8, currency: u8, remove: bool },
    Reads { sme: u8, id: u64, itag: u8, start: u32, limit: u32, ltag: u8, currency: u8 },
    ProposeUpgrade { admin: u8, hash: u8 },
    ExecuteUpgrade { admin: u8 },
    GetAuditLog { page: u32, page_size: u32, ptag: u8, stag: u8 },
}

fn batch_input(p: &Protocol, m: &MintArgs) -> BatchInvoiceInput {
    let env = &p.env;
    return BatchInvoiceInput {
        debtor_hash: gen::bytes32(env, m.hash),
        amount: gen::amount(m.amount, m.atag),
        currency: gen::symbol(env, m.currency),
        due_date: gen::ts_or_id(m.due_date, m.dtag),
        ipfs_cid: gen::text(env, m.cid),
        risk_score: gen::small_u32(m.score, m.stag),
        notes: if m.with_notes { Some(gen::text(env, m.cid)) } else { None },
    };
}

pub fn run(data: &[u8]) {
    let p = Protocol::deploy();
    super::drive::<Op, _>(data, |op| {
        let c = &p.invoice_nft;
        let env = &p.env;
        match op {
            Op::Initialize { admin, ac } => {
                let _ = c.try_initialize(&p.actor(admin), &p.actor(ac));
            }
            Op::SetRiskRegistry { admin, rr } => {
                let _ = c.try_set_risk_registry(&p.actor(admin), &p.actor(rr));
            }
            Op::Migrate { admin } => {
                let _ = c.try_migrate(&p.actor(admin));
            }
            Op::SetAuthorizedCallers { admin, mp, pool } => {
                let _ = c.try_set_authorized_callers(&p.actor(admin), &p.actor(mp), &p.actor(pool));
            }
            Op::MintInvoice(m) => {
                let notes = if m.with_notes { Some(gen::text(env, m.cid)) } else { None };
                let _ = c.try_mint_invoice(
                    &p.actor(m.sme),
                    &gen::bytes32(env, m.hash),
                    &gen::amount(m.amount, m.atag),
                    &gen::symbol(env, m.currency),
                    &gen::ts_or_id(m.due_date, m.dtag),
                    &gen::text(env, m.cid),
                    &gen::small_u32(m.score, m.stag),
                    &notes,
                );
            }
            Op::MintBatch { sme, a, b, count } => {
                let mut v: SVec<BatchInvoiceInput> = SVec::new(env);
                for _ in 0..(count % 4) {
                    v.push_back(batch_input(&p, &a));
                }
                v.push_back(batch_input(&p, &b));
                let _ = c.try_mint_invoices_batch(&p.actor(sme), &v);
            }
            Op::AmendInvoice { sme, id, itag, hash, amount, atag, due, dtag, cid, score, stag } => {
                let _ = c.try_amend_invoice(
                    &p.actor(sme),
                    &gen::ts_or_id(id, itag),
                    &gen::bytes32(env, hash),
                    &gen::amount(amount, atag),
                    &gen::ts_or_id(due, dtag),
                    &gen::text(env, cid),
                    &gen::small_u32(score, stag),
                );
            }
            Op::WithdrawInvoice { sme, id, itag } => {
                let _ = c.try_withdraw_invoice(&p.actor(sme), &gen::ts_or_id(id, itag));
            }
            Op::Transition { kind, caller, id, itag } => {
                let a = p.actor(caller);
                let i = gen::ts_or_id(id, itag);
                match kind % 5 {
                    0 => { let _ = c.try_set_created(&a, &i); }
                    1 => { let _ = c.try_set_listed(&a, &i); }
                    2 => { let _ = c.try_set_funded(&a, &i); }
                    3 => { let _ = c.try_set_repaid(&a, &i); }
                    _ => { let _ = c.try_set_defaulted(&a, &i); }
                }
            }
            Op::CommitMetadataHash { sme, id, itag, hash } => {
                let _ = c.try_commit_metadata_hash(
                    &p.actor(sme),
                    &gen::ts_or_id(id, itag),
                    &gen::bytes32(env, hash),
                );
            }
            Op::Freeze { admin, id, itag, unfreeze } => {
                let i = gen::ts_or_id(id, itag);
                if unfreeze {
                    let _ = c.try_unfreeze_invoice(&p.actor(admin), &i);
                } else {
                    let _ = c.try_freeze_invoice(&p.actor(admin), &i);
                }
            }
            Op::FreezeSme { admin, sme, max, mtag, unfreeze } => {
                let m = gen::small_u32(max, mtag);
                if unfreeze {
                    let _ = c.try_unfreeze_sme_invoices(&p.actor(admin), &p.actor(sme), &m);
                } else {
                    let _ = c.try_freeze_sme_invoices(&p.actor(admin), &p.actor(sme), &m);
                }
            }
            Op::RefreshRiskScore { caller, id, itag } => {
                let _ = c.try_refresh_risk_score(&p.actor(caller), &gen::ts_or_id(id, itag));
            }
            Op::Currency { admin, currency, remove } => {
                let s = gen::symbol(env, currency);
                if remove {
                    let _ = c.try_remove_allowed_currency(&p.actor(admin), &s);
                } else {
                    let _ = c.try_add_allowed_currency(&p.actor(admin), &s);
                }
            }
            Op::Reads { sme, id, itag, start, limit, ltag, currency } => {
                let i = gen::ts_or_id(id, itag);
                let _ = c.try_get_invoice(&i);
                let _ = c.try_is_invoice_frozen(&i);
                let _ = c.try_next_id();
                let _ = c.try_invoice_count();
                let _ = c.try_get_outstanding_exposure(&p.actor(sme));
                let _ = c.try_get_sme_invoice_ids(
                    &p.actor(sme),
                    &gen::small_u32(start, ltag),
                    &gen::small_u32(limit, ltag),
                );
                let _ = c.try_is_currency_allowed(&gen::symbol(env, currency));
            }
            Op::ProposeUpgrade { admin, hash } => {
                let _ = c.try_propose_upgrade(&p.actor(admin), &gen::hash32(env, hash));
            }
            Op::ExecuteUpgrade { admin } => {
                let _ = c.try_execute_upgrade(&p.actor(admin));
            }
            Op::GetAuditLog { page, page_size, ptag, stag } => {
                let _ = c.try_get_audit_log(&gen::small_u32(page, ptag), &gen::small_u32(page_size, stag));
            }
        }
    });
}
