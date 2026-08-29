use arbitrary::Arbitrary;
use soroban_sdk::Vec as SVec;

use crate::gen;
use crate::harness::Protocol;

#[derive(Arbitrary, Debug)]
enum Op {
    Initialize { admin: u8 },
    Pause { admin: u8 },
    Unpause { admin: u8 },
    GrantRole { admin: u8, target: u8, role: u8 },
    RevokeRole { admin: u8, target: u8 },
    TransferAdmin { current: u8, new: u8 },
    ConfigureMultisig { admin: u8, signers: [u8; 4], threshold: u32 },
    ProposeAction { proposer: u8, action: u8, arg_addr: u8, arg_role: u8 },
    ApproveAction { approver: u8, proposal_id: u64, ptag: u8 },
    ExecuteAction { executor: u8, proposal_id: u64, ptag: u8 },
    ProposeParameterChange { proposer: u8, key: u8, value: u32, vtag: u8 },
    VoteParameterChange { signer: u8, proposal_id: u64, ptag: u8 },
    ExecuteParameterChange { caller: u8, proposal_id: u64, ptag: u8 },
    GetParameter { key: u8 },
    GetProposal { proposal_id: u64, ptag: u8 },
    Reads { addr: u8, role: u8 },
    ProposeUpgrade { admin: u8, hash: u8 },
    ExecuteUpgrade { admin: u8 },
    GetAuditLog { page: u32, page_size: u32, ptag: u8, stag: u8 },
}

fn admin_action(p: &Protocol, action: u8, arg_addr: u8, arg_role: u8) -> kora_shared::types::AdminAction {
    use kora_shared::types::AdminAction;
    return match action % 5 {
        0 => AdminAction::Pause,
        1 => AdminAction::Unpause,
        2 => AdminAction::GrantRole(p.actor(arg_addr), arg_role as u32),
        3 => AdminAction::RevokeRole(p.actor(arg_addr)),
        _ => AdminAction::TransferAdmin(p.actor(arg_addr)),
    };
}

pub fn run(data: &[u8]) {
    let p = Protocol::deploy();
    super::drive::<Op, _>(data, |op| {
        let c = &p.access_control;
        let env = &p.env;
        match op {
            Op::Initialize { admin } => {
                let _ = c.try_initialize(&p.actor(admin));
            }
            Op::Pause { admin } => {
                let _ = c.try_pause(&p.actor(admin));
            }
            Op::Unpause { admin } => {
                let _ = c.try_unpause(&p.actor(admin));
            }
            Op::GrantRole { admin, target, role } => {
                let _ = c.try_grant_role(&p.actor(admin), &p.actor(target), &gen::role(role));
            }
            Op::RevokeRole { admin, target } => {
                let _ = c.try_revoke_role(&p.actor(admin), &p.actor(target));
            }
            Op::TransferAdmin { current, new } => {
                let _ = c.try_transfer_admin(&p.actor(current), &p.actor(new));
            }
            Op::ConfigureMultisig { admin, signers, threshold } => {
                let mut v: SVec<soroban_sdk::Address> = SVec::new(env);
                for s in signers {
                    v.push_back(p.actor(s));
                }
                let _ = c.try_configure_multisig(&p.actor(admin), &v, &threshold);
            }
            Op::ProposeAction { proposer, action, arg_addr, arg_role } => {
                let a = admin_action(&p, action, arg_addr, arg_role);
                let _ = c.try_propose_action(&p.actor(proposer), &a);
            }
            Op::ApproveAction { approver, proposal_id, ptag } => {
                let _ = c.try_approve_action(&p.actor(approver), &gen::ts_or_id(proposal_id, ptag));
            }
            Op::ExecuteAction { executor, proposal_id, ptag } => {
                let _ = c.try_execute_action(&p.actor(executor), &gen::ts_or_id(proposal_id, ptag));
            }
            Op::ProposeParameterChange { proposer, key, value, vtag } => {
                let _ = c.try_propose_parameter_change(
                    &p.actor(proposer),
                    &gen::parameter_key(key),
                    &gen::small_u32(value, vtag),
                );
            }
            Op::VoteParameterChange { signer, proposal_id, ptag } => {
                let _ = c.try_vote_parameter_change(
                    &p.actor(signer),
                    &gen::ts_or_id(proposal_id, ptag),
                );
            }
            Op::ExecuteParameterChange { caller, proposal_id, ptag } => {
                let _ = c.try_execute_parameter_change(
                    &p.actor(caller),
                    &gen::ts_or_id(proposal_id, ptag),
                );
            }
            Op::GetParameter { key } => {
                let _ = c.try_get_parameter(&gen::parameter_key(key));
            }
            Op::GetProposal { proposal_id, ptag } => {
                let _ = c.try_get_proposal(&gen::ts_or_id(proposal_id, ptag));
                let _ = c.try_get_parameter_proposal(&gen::ts_or_id(proposal_id, ptag));
                let _ = c.try_get_multisig_config();
            }
            Op::Reads { addr, role } => {
                let _ = c.try_is_paused();
                let _ = c.try_get_role(&p.actor(addr));
                let _ = c.try_has_role(&p.actor(addr), &gen::role(role));
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
