use arbitrary::Arbitrary;

use crate::gen;
use crate::harness::Protocol;

#[derive(Arbitrary, Debug)]
enum Op {
    Initialize { admin: u8, fee_bps: u32, ftag: u8 },
    SetFeeBps { admin: u8, fee_bps: u32, ftag: u8 },
    WhitelistToken { admin: u8, token: u8 },
    CollectFee { token: u8, amount: i128, atag: u8 },
    Withdraw { admin: u8, token: u8, recipient: u8, amount: i128, atag: u8 },
    EmergencyWithdraw { admin: u8, token: u8, recipient: u8 },
    ProposeWithdrawalCap { admin: u8, cap: i128, ctag: u8 },
    ExecuteWithdrawalCap { admin: u8 },
    Reads { token: u8 },
    ProposeUpgrade { admin: u8, hash: u8 },
    ExecuteUpgrade { admin: u8 },
    GetAuditLog { page: u32, page_size: u32, ptag: u8, stag: u8 },
}

pub fn run(data: &[u8]) {
    let p = Protocol::deploy();
    super::drive::<Op, _>(data, |op| {
        let c = &p.treasury;
        let env = &p.env;
        match op {
            Op::Initialize { admin, fee_bps, ftag } => {
                let _ = c.try_initialize(&p.actor(admin), &gen::small_u32(fee_bps, ftag));
            }
            Op::SetFeeBps { admin, fee_bps, ftag } => {
                let _ = c.try_set_fee_bps(&p.actor(admin), &gen::small_u32(fee_bps, ftag));
            }
            Op::WhitelistToken { admin, token } => {
                let _ = c.try_whitelist_token(&p.actor(admin), &p.actor(token));
            }
            Op::CollectFee { token, amount, atag } => {
                let _ = c.try_collect_fee(&p.actor(token), &gen::amount(amount, atag));
            }
            Op::Withdraw { admin, token, recipient, amount, atag } => {
                let _ = c.try_withdraw(
                    &p.actor(admin),
                    &p.actor(token),
                    &p.actor(recipient),
                    &gen::amount(amount, atag),
                );
            }
            Op::EmergencyWithdraw { admin, token, recipient } => {
                let _ = c.try_emergency_withdraw(&p.actor(admin), &p.actor(token), &p.actor(recipient));
            }
            Op::ProposeWithdrawalCap { admin, cap, ctag } => {
                let _ = c.try_propose_withdrawal_cap(&p.actor(admin), &gen::amount(cap, ctag));
            }
            Op::ExecuteWithdrawalCap { admin } => {
                let _ = c.try_execute_withdrawal_cap(&p.actor(admin));
            }
            Op::Reads { token } => {
                let _ = c.try_get_withdrawal_cap();
                let _ = c.try_get_fee_bps();
                let _ = c.try_get_balance(&p.actor(token));
                let _ = c.try_get_collected(&p.actor(token));
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
