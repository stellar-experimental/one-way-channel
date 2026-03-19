#![no_std]
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, symbol_short, token, xdr::ToXdr, Address, Bytes, BytesN, Env, Symbol};

mod events;
use events::*;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotAborting = 1,
    AbortPeriodNotElapsed = 2,
    NotClosed = 3,
    NotRefundable = 4,
}

#[contracttype]
pub enum DataKey {
    Token,
    From,
    CommitmentKey,
    To,
    AbortLedgerCount,
    Abort,
    Closed,
}

#[contracttype]
pub struct Commitment {
    pub prefix: Symbol,
    pub channel: Address,
    pub amount: i128,
}

impl Commitment {
    fn new(env: &Env, amount: i128) -> Self {
        Commitment {
            prefix: symbol_short!("chancmmt"),
            channel: env.current_contract_address(),
            amount,
        }
    }

    fn into_bytes(self, env: &Env) -> Bytes {
        self.to_xdr(env)
    }

    fn verify(self, env: &Env, sig: &BytesN<64>) {
        let commitment_key: BytesN<32> = env.storage().instance().get(&DataKey::CommitmentKey).unwrap();
        let payload = self.into_bytes(env);
        env.crypto().ed25519_verify(&commitment_key, &payload, sig);
    }
}

#[contracttype]
pub struct Abort {
    pub abort_at_ledger: u32,
}

#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
    pub fn __constructor(env: Env, token: Address, from: Address, commitment_key: BytesN<32>, to: Address, amount: i128, abort_ledger_count: u32) {
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::From, &from);
        env.storage().instance().set(&DataKey::CommitmentKey, &commitment_key);
        env.storage().instance().set(&DataKey::To, &to);
        env.storage().instance().set(&DataKey::AbortLedgerCount, &abort_ledger_count);
        Self::top_up(env.clone(), amount);
        env.events().publish_event(&OpenEvent {
            from,
            commitment_key,
            to,
            token,
            amount,
            abort_ledger_count,
        });
    }

    /// Top up the channel with the stored token from the stored from address.
    /// Called by anyone, but only the from address is debited.
    ///
    /// # Auth
    /// - `from`: required.
    pub fn top_up(env: Env, amount: i128) {
        if amount > 0 {
            let from: Address = env.storage().instance().get(&DataKey::From).unwrap();
            from.require_auth();
            Self::token_client(&env).transfer(&from, &env.current_contract_address(), &amount);
        }
    }

    /// Returns the total amount deposited in the channel.
    /// Called by anyone.
    ///
    /// # Auth
    /// None.
    pub fn balance_deposited(env: Env) -> i128 {
        Self::token_client(&env).balance(&env.current_contract_address())
    }

    /// Returns the commitment payload that needs to be signed by the
    /// commitment_key. The signed commitment can be passed to close.
    /// Called by anyone.
    ///
    /// # Auth
    /// None.
    pub fn prepare_commitment(env: Env, amount: i128) -> Bytes {
        Commitment::new(&env, amount).into_bytes(&env)
    }

    /// Start aborting the channel. If undisputed, abort_finish will result in a
    /// full refund to the funder. The recipient can dispute by calling
    /// close with a commitment during the waiting period.
    /// Called by the funder (from).
    ///
    /// # Auth
    /// - `from`: required.
    pub fn abort_start(env: Env) {
        let from: Address = env.storage().instance().get(&DataKey::From).unwrap();
        from.require_auth();
        let abort_ledger_count: u32 = env.storage().instance().get(&DataKey::AbortLedgerCount).unwrap();
        let abort_at_ledger = env.ledger().sequence() + abort_ledger_count;
        env.storage().instance().set(&DataKey::Abort, &Abort { abort_at_ledger });
        env.events().publish_event(&AbortStartEvent { abort_at_ledger });
    }

    /// Finish the abort after the abort_at_ledger has been reached.
    /// Closes the channel with amount 0, resulting in a full refund to the funder.
    /// Called by anyone.
    ///
    /// # Auth
    /// None.
    pub fn abort_finish(env: Env) -> Result<(), Error> {
        let abort: Abort = env.storage().instance().get(&DataKey::Abort).ok_or(Error::NotAborting)?;

        if env.ledger().sequence() < abort.abort_at_ledger {
            return Err(Error::AbortPeriodNotElapsed);
        }

        env.storage().instance().remove(&DataKey::Abort);
        env.storage().instance().set(&DataKey::Closed, &0i128);
        env.events().publish_event(&ClosedEvent { amount: 0 });
        Ok(())
    }

    /// Close the channel by submitting a commitment. No waiting period.
    /// Called by the recipient (to).
    ///
    /// # Auth
    /// - `to`: required.
    /// - Commitment signature serves as commitment_key authorization.
    pub fn close(env: Env, amount: i128, sig: BytesN<64>) {
        let to: Address = env.storage().instance().get(&DataKey::To).unwrap();
        to.require_auth();
        Commitment::new(&env, amount).verify(&env, &sig);
        env.storage().instance().remove(&DataKey::Abort);
        env.storage().instance().set(&DataKey::Closed, &amount);
        env.events().publish_event(&ClosedEvent { amount });
    }

    /// Withdraw the authorized amount to `to` after the channel is closed.
    /// Called by anyone.
    ///
    /// # Auth
    /// None.
    pub fn withdraw(env: Env) -> Result<(), Error> {
        let amount: i128 = env.storage().instance().get(&DataKey::Closed).ok_or(Error::NotClosed)?;
        if amount == 0 {
            return Err(Error::NotClosed);
        }
        let to: Address = env.storage().instance().get(&DataKey::To).unwrap();
        env.storage().instance().set(&DataKey::Closed, &0i128);
        Self::token_client(&env).transfer(&env.current_contract_address(), &to, &amount);
        env.events().publish_event(&WithdrawEvent { to, amount });
        Ok(())
    }

    /// Refund the funder's portion of the balance.
    /// Can be called after the channel is closed. The refundable amount is
    /// the balance minus the closed amount (if not yet withdrawn).
    /// Called by the funder (from).
    ///
    /// # Auth
    /// - `from`: required.
    pub fn refund(env: Env) -> Result<(), Error> {
        let closed_amount: i128 = env.storage().instance().get(&DataKey::Closed).ok_or(Error::NotClosed)?;
        let from: Address = env.storage().instance().get(&DataKey::From).unwrap();
        from.require_auth();
        let tc = Self::token_client(&env);
        let balance = tc.balance(&env.current_contract_address());
        let refundable = balance - closed_amount;
        if refundable > 0 {
            tc.transfer(&env.current_contract_address(), &from, &refundable);
            env.events().publish_event(&RefundEvent { from, amount: refundable });
        }
        Ok(())
    }
}

impl Contract {
    fn token_client(env: &Env) -> token::Client<'_> {
        let token: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        token::Client::new(env, &token)
    }
}

mod test;
