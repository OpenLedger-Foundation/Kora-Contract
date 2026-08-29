use arbitrary::Arbitrary;

use crate::gen;
use crate::harness::Protocol;

#[derive(Arbitrary, Debug)]
enum Op {
    Initialize { admin: u8, nft: u8, token: u8, min_stake: i128, mtag: u8, slash_bps: u32, stag: u8 },
    TransferAdmin { admin: u8, new: u8 },
    SetInvoiceNft { admin: u8, nft: u8 },
    AddVerifier { admin: u8, verifier: u8, stake: i128, stag: u8 },
    RemoveVerifier { admin: u8, verifier: u8 },
    AddSubAccount { primary: u8, sub: u8 },
    RemoveSubAccount { primary: u8, sub: u8 },
    RegisterSme { verifier: u8, sme: u8, score: u32, stag: u8, attested: bool },
    UpdateSmeScore { verifier: u8, sme: u8, score: u32, stag: u8 },
    SetCreditLimit { verifier: u8, sme: u8, limit: i128, ltag: u8 },
    IncrementInvoiceCount { caller: u8, sme: u8 },
    RecordDefault { admin: u8, sme: u8 },
    SetDebtorScore { verifier: u8, hash: u8, score: u32, stag: u8 },
    Reads { who: u8, hash: u8 },
    ProposeUpgrade { admin: u8, hash: u8 },
    ExecuteUpgrade { admin: u8 },
    GetAuditLog { page: u32, page_size: u32, ptag: u8, stag: u8 },
}

pub fn run(data: &[u8]) {
    let p = Protocol::deploy();
    super::drive::<Op, _>(data, |op| {
        let c = &p.risk_registry;
        let env = &p.env;
        match op {
            Op::Initialize { admin, nft, token, min_stake, mtag, slash_bps, stag } => {
                let _ = c.try_initialize(
                    &p.actor(admin),
                    &p.actor(nft),
                    &p.actor(token),
                    &gen::amount(min_stake, mtag),
                    &gen::small_u32(slash_bps, stag),
                );
            }
            Op::TransferAdmin { admin, new } => {
                let _ = c.try_transfer_admin(&p.actor(admin), &p.actor(new));
            }
            Op::SetInvoiceNft { admin, nft } => {
                let _ = c.try_set_invoice_nft(&p.actor(admin), &p.actor(nft));
            }
            Op::AddVerifier { admin, verifier, stake, stag } => {
                let _ = c.try_add_verifier(&p.actor(admin), &p.actor(verifier), &gen::amount(stake, stag));
            }
            Op::RemoveVerifier { admin, verifier } => {
                let _ = c.try_remove_verifier(&p.actor(admin), &p.actor(verifier));
            }
            Op::AddSubAccount { primary, sub } => {
                let _ = c.try_add_sub_account(&p.actor(primary), &p.actor(sub));
            }
            Op::RemoveSubAccount { primary, sub } => {
                let _ = c.try_remove_sub_account(&p.actor(primary), &p.actor(sub));
            }
            Op::RegisterSme { verifier, sme, score, stag, attested } => {
                let _ = c.try_register_sme(
                    &p.actor(verifier),
                    &p.actor(sme),
                    &gen::small_u32(score, stag),
                    &attested,
                );
            }
            Op::UpdateSmeScore { verifier, sme, score, stag } => {
                let _ = c.try_update_sme_score(&p.actor(verifier), &p.actor(sme), &gen::small_u32(score, stag));
            }
            Op::SetCreditLimit { verifier, sme, limit, ltag } => {
                let _ = c.try_set_credit_limit(&p.actor(verifier), &p.actor(sme), &gen::amount(limit, ltag));
            }
            Op::IncrementInvoiceCount { caller, sme } => {
                let _ = c.try_increment_invoice_count(&p.actor(caller), &p.actor(sme));
            }
            Op::RecordDefault { admin, sme } => {
                let _ = c.try_record_default(&p.actor(admin), &p.actor(sme));
            }
            Op::SetDebtorScore { verifier, hash, score, stag } => {
                let _ = c.try_set_debtor_score(
                    &p.actor(verifier),
                    &gen::bytes32(env, hash),
                    &gen::small_u32(score, stag),
                );
            }
            Op::Reads { who, hash } => {
                let a = p.actor(who);
                let _ = c.try_get_sme_profile(&a);
                let _ = c.try_is_verified_sme(&a);
                let _ = c.try_is_compliance_attested(&a);
                let _ = c.try_get_verifier_stake(&a);
                let _ = c.try_get_verifier_reputation(&a);
                let _ = c.try_is_verifier(&a);
                let _ = c.try_get_primary_verifier(&a);
                let _ = c.try_is_sub_account(&a);
                let _ = c.try_get_debtor_score(&gen::bytes32(env, hash));
                let _ = c.try_get_admin();
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
