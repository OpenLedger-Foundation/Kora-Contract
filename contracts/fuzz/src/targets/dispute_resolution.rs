//! Byte-stream state-machine oracle for resolution and its independent escrow
//! companion. Unexpected host failures are findings, not ignored `try_*` errors.
use kora_dispute_escrow::{EscrowError as EE, DISPUTE_TIMEOUT};
use kora_dispute_resolution::DisputeResolutionError as RE;
use soroban_sdk::{
    testutils::Ledger, token, xdr::ScErrorType, Address, Env, Error, IntoVal, String, Symbol, Val, Vec,
};

use crate::harness::dispute::DisputeHarness;

const SLOTS: usize = 4;
const WINDOW: u64 = 7 * 86_400;

/// These mirror `Dispute.resolved` and `Dispute.upheld`, including both
/// absorbing outcomes. Evidence changes an open record, not its status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Absent,
    Open,
    Upheld,
    Rejected,
}

#[derive(Clone)]
struct Model {
    state: State,
    challenger: usize,
    evidence: Option<String>,
    opened_at: u64,
    resolved_at: u64,
}

impl Model {
    fn absent() -> Self {
        Self {
            state: State::Absent,
            challenger: 0,
            evidence: None,
            opened_at: 0,
            resolved_at: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct EscrowModel {
    challenger: usize,
    amount: i128,
    opened_at: u64,
    resolved: bool,
    paid_to_pool: bool,
}

#[derive(Clone, Copy)]
enum Expected {
    Success,
    Contract(u32),
    MissingAuth,
}

/// Keep raw Soroban errors so the expected authentication failure cannot hide
/// a panic, budget trap, conversion failure, or another unexpected host error.
fn invoke(env: &Env, contract: &Address, name: &str, args: Vec<Val>, expected: Expected) {
    let result = env.try_invoke_contract::<(), Error>(contract, &Symbol::new(env, name), args);
    match expected {
        Expected::Success => assert!(matches!(result, Ok(Ok(()))), "{name}: {result:?}"),
        Expected::Contract(code) => {
            assert_eq!(result, Err(Ok(Error::from_contract_error(code))), "{name}");
        }
        Expected::MissingAuth => match result {
            Err(Ok(error)) => assert!(error.is_type(ScErrorType::Auth), "{name}: {error:?}"),
            other => panic!("{name}: expected auth failure, got {other:?}"),
        },
    }
}

fn expected(error: Option<u32>, authenticated: bool) -> Expected {
    if !authenticated {
        Expected::MissingAuth
    } else if let Some(code) = error {
        Expected::Contract(code)
    } else {
        Expected::Success
    }
}

fn assert_invariants(
    p: &DisputeHarness,
    states: &[Model; SLOTS],
    escrows: &[Option<EscrowModel>; SLOTS],
) {
    let mut locked = 0;
    let mut paid = 0;
    for slot in 0..SLOTS {
        let id = slot as u64;
        let model = &states[slot];
        let stored = p.resolution.get_dispute(&id);
        assert_eq!(stored.is_none(), model.state == State::Absent);
        assert_eq!(p.resolution.has_open_dispute(&id), model.state == State::Open);
        if let Some(stored) = stored {
            assert_eq!(stored.invoice_id, id);
            assert_eq!(stored.challenger, p.actors[model.challenger]);
            assert_eq!(stored.evidence_cid, model.evidence);
            assert_eq!(stored.opened_at, model.opened_at);
            assert_eq!(stored.resolved_at, model.resolved_at);
            assert_eq!(
                stored.resolved,
                matches!(model.state, State::Upheld | State::Rejected)
            );
            assert_eq!(stored.upheld, model.state == State::Upheld);
        }
        let escrow = p.escrow.get_dispute(&id);
        assert_eq!(escrow.is_none(), escrows[slot].is_none());
        if let Some(model) = escrows[slot] {
            let escrow = escrow.unwrap();
            assert_eq!(escrow.invoice_id, id);
            assert_eq!(escrow.challenger, p.actors[model.challenger]);
            assert_eq!(escrow.payer, p.actors[2]);
            assert_eq!(escrow.token, p.token);
            assert_eq!(escrow.amount, model.amount);
            assert_eq!(escrow.opened_at, model.opened_at);
            assert_eq!(escrow.resolved, model.resolved);
            assert_eq!(escrow.paid_to_pool, model.paid_to_pool);
            assert_eq!(p.pool.releases(&id), u32::from(model.resolved));
            if !model.resolved {
                locked += model.amount;
            }
            if model.paid_to_pool {
                paid += model.amount;
            }
        } else {
            assert_eq!(p.pool.releases(&id), 0);
        }
    }
    let token = token::Client::new(&p.env, &p.token);
    assert_eq!(token.balance(&p.escrow.address), locked);
    assert_eq!(token.balance(&p.pool.address), paid);
    assert_eq!(token.balance(&p.actors[2]), 1_000 - locked - paid);
    assert_eq!(token.balance(&p.actors[0]), 0);
    assert_eq!(token.balance(&p.actors[1]), 0);
    assert_eq!(token.balance(&p.actors[3]), 0);
}

/// Decode four-byte operations: opcode, invoice slot, actor/auth, argument.
/// Actor bit 7 omits all transaction auth; low two bits choose the actor.
/// Trailing partial operations are ignored; at most MAX_OPS are executed.
pub fn run(data: &[u8]) {
    let p = DisputeHarness::deploy();
    let env = &p.env;
    let mut states: [Model; SLOTS] = core::array::from_fn(|_| Model::absent());
    let mut escrows = [None; SLOTS];
    let mut paused = false;
    let mut governance = true;
    assert_invariants(&p, &states, &escrows);
    for bytes in data.chunks_exact(4).take(crate::MAX_OPS) {
        let op = bytes[0] % 10;
        let slot = bytes[1] as usize % SLOTS;
        let actor = bytes[2] as usize % p.actors.len();
        let authenticated = bytes[2] & 0x80 == 0;
        let arg = bytes[3];
        let id = slot as u64;
        let caller = &p.actors[actor];
        let now = env.ledger().timestamp();
        let model = &mut states[slot];
        if authenticated {
            env.mock_all_auths();
        } else {
            env.mock_auths(&[]);
        }
        match op {
            0 => {
                let error = if paused {
                    Some(RE::ProtocolPaused)
                } else if model.state != State::Absent {
                    Some(RE::DisputeAlreadyOpen)
                } else {
                    None
                };
                invoke(
                    env,
                    &p.resolution.address,
                    "open_dispute",
                    (caller.clone(), id).into_val(env),
                    expected(error.map(|e| e as u32), authenticated),
                );
                if authenticated && error.is_none() {
                    *model = Model {
                        state: State::Open,
                        challenger: actor,
                        evidence: None,
                        opened_at: now,
                        resolved_at: 0,
                    };
                }
            }
            1 => {
                let cid = String::from_str(
                    env,
                    match arg % 3 {
                        0 => "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
                        1 => "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG",
                        _ => "invalid",
                    },
                );
                let error = if model.state == State::Absent {
                    Some(RE::DisputeNotFound)
                } else if model.state != State::Open {
                    Some(RE::DisputeAlreadyResolved)
                } else if actor != model.challenger {
                    Some(RE::NotDisputeChallenger)
                } else if arg % 3 == 2 {
                    Some(RE::InvalidCid)
                } else {
                    None
                };
                invoke(
                    env,
                    &p.resolution.address,
                    "submit_evidence",
                    (caller.clone(), id, cid.clone()).into_val(env),
                    expected(error.map(|e| e as u32), authenticated),
                );
                if authenticated && error.is_none() {
                    model.evidence = Some(cid);
                }
            }
            2 => {
                let upheld = arg & 1 != 0;
                let error = if !governance || actor != 1 {
                    Some(RE::NotGovernance)
                } else if model.state == State::Absent {
                    Some(RE::DisputeNotFound)
                } else if model.state != State::Open {
                    Some(RE::DisputeAlreadyResolved)
                } else if now > model.opened_at + WINDOW {
                    Some(RE::DisputeWindowExpired)
                } else {
                    None
                };
                invoke(
                    env,
                    &p.resolution.address,
                    "resolve_dispute",
                    (caller.clone(), id, upheld).into_val(env),
                    expected(error.map(|e| e as u32), authenticated),
                );
                if authenticated && error.is_none() {
                    model.state = if upheld { State::Upheld } else { State::Rejected };
                    model.resolved_at = now;
                }
            }
            3 => {
                // Repeated bounded advances cover before/at/after each deadline.
                env.ledger().with_mut(|ledger| {
                    ledger.timestamp += match arg % 5 {
                        0 => 0,
                        1 => 1,
                        2 => WINDOW - 1,
                        3 => WINDOW,
                        _ => WINDOW + 1,
                    }
                });
            }
            4 => {
                paused = arg & 1 != 0;
                governance = arg & 2 == 0;
                p.access.configure(&p.actors[1], &paused, &governance);
            }
            5 => {
                let amount = [-1, 0, 1, 100, 101][arg as usize % 5];
                let error = if amount <= 0 {
                    Some(EE::InvalidAmount)
                } else if escrows[slot].is_some() {
                    Some(EE::AlreadyDisputed)
                } else if amount > 100 {
                    Some(EE::InvalidAmount)
                } else if actor != 1 && actor != 2 {
                    // Outsider (actor 3) and admin (actor 0) cannot open escrow;
                    // only the signer/investor (1) and payer (2) are eligible.
                    Some(EE::Unauthorized)
                } else {
                    None
                };
                invoke(
                    env,
                    &p.escrow.address,
                    "open_dispute",
                    (caller.clone(), p.actors[2].clone(), id, amount).into_val(env),
                    expected(error.map(|e| e as u32), authenticated),
                );
                if authenticated && error.is_none() {
                    escrows[slot] = Some(EscrowModel {
                        challenger: actor,
                        amount,
                        opened_at: now,
                        resolved: false,
                        paid_to_pool: false,
                    });
                }
            }
            6 | 7 => {
                let pay_pool = op == 6 && arg & 1 != 0;
                let escrow = escrows[slot];
                let error = if op == 6 && actor != 0 {
                    Some(EE::Unauthorized)
                } else {
                    match escrow {
                        None => Some(EE::NotFound),
                        Some(m) if m.resolved => Some(EE::Resolved),
                        Some(m) if op == 6 && now >= m.opened_at + DISPUTE_TIMEOUT => {
                            Some(EE::TimeoutExpired)
                        }
                        Some(m) if op == 7 && now < m.opened_at + DISPUTE_TIMEOUT => {
                            Some(EE::TimeoutNotReached)
                        }
                        Some(_) => None,
                    }
                };
                let args = if op == 6 {
                    (caller.clone(), id, pay_pool).into_val(env)
                } else {
                    (id,).into_val(env)
                };
                // Timeout is permissionless and succeeds even without auth.
                let authorized = op == 7 || authenticated;
                invoke(
                    env,
                    &p.escrow.address,
                    if op == 6 { "resolve_dispute" } else { "resolve_timeout" },
                    args,
                    expected(error.map(|e| e as u32), authorized),
                );
                if authorized && error.is_none() {
                    let escrow = escrows[slot].as_mut().unwrap();
                    escrow.resolved = true;
                    escrow.paid_to_pool = pay_pool;
                }
            }
            8 => {
                // Initialization is unauthenticated but cannot replace authority.
                invoke(
                    env,
                    &p.resolution.address,
                    "initialize",
                    (caller.clone(), p.access.address.clone()).into_val(env),
                    Expected::Contract(RE::AlreadyInitialized as u32),
                );
                invoke(
                    env,
                    &p.escrow.address,
                    "initialize",
                    (caller.clone(), p.pool.address.clone()).into_val(env),
                    Expected::Contract(EE::AlreadyInitialized as u32),
                );
            }
            _ => {} // Reads are exercised for every slot after every opcode.
        }
        // This includes the unchanged state of every rejected invocation, both
        // terminal outcomes, and invoices other than the one just targeted.
        assert_invariants(&p, &states, &escrows);
    }
}
