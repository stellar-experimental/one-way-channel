#![no_std]
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, symbol_short, token, xdr::ToXdr, Address, Bytes, BytesN, Env, Symbol};

mod events;
use events::*;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotClosed = 1,
    ClosePeriodNotElapsed = 2,
}

#[contracttype]
pub enum DataKey {
    Token,
    From,
    CommitmentKey,
    To,
    CloseLedgerCount,
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
pub struct Closed {
    pub amount: i128,
    pub effective_at_ledger: u32,
}

#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
    pub fn __constructor(env: Env, token: Address, from: Address, commitment_key: BytesN<32>, to: Address, amount: i128, close_ledger_count: u32) {
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::From, &from);
        env.storage().instance().set(&DataKey::CommitmentKey, &commitment_key);
        env.storage().instance().set(&DataKey::To, &to);
        env.storage().instance().set(&DataKey::CloseLedgerCount, &close_ledger_count);
        Self::top_up(env.clone(), amount);
        env.events().publish_event(&OpenEvent {
            from,
            commitment_key,
            to,
            token,
            amount,
            close_ledger_count,
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
    /// commitment_key. The signed commitment can be passed to
    /// close_with_commitment.
    /// Called by anyone.
    ///
    /// # Auth
    /// None.
    pub fn prepare_commitment(env: Env, amount: i128) -> Bytes {
        Commitment::new(&env, amount).into_bytes(&env)
    }

    /// Close the channel with the given amount, effective after a waiting
    /// period. The recipient can update the amount by calling
    /// close_with_commitment before the close becomes effective.
    /// Called by the funder (from).
    ///
    /// # Auth
    /// - `from`: required.
    pub fn close(env: Env, amount: i128) {
        let from: Address = env.storage().instance().get(&DataKey::From).unwrap();
        from.require_auth();
        let close_ledger_count: u32 = env.storage().instance().get(&DataKey::CloseLedgerCount).unwrap();
        let effective_at_ledger = env.ledger().sequence() + close_ledger_count;
        env.storage().instance().set(&DataKey::Closed, &Closed { amount, effective_at_ledger });
        env.events().publish_event(&CloseEvent { effective_at_ledger });
    }

    /// Close the channel by submitting a commitment. Effective immediately.
    /// Called by the recipient (to).
    ///
    /// # Auth
    /// - `to`: required.
    /// - Commitment signature serves as commitment_key authorization.
    pub fn close_with_commitment(env: Env, amount: i128, sig: BytesN<64>) {
        let to: Address = env.storage().instance().get(&DataKey::To).unwrap();
        to.require_auth();
        Commitment::new(&env, amount).verify(&env, &sig);
        let effective_at_ledger = env.ledger().sequence();
        env.storage().instance().set(&DataKey::Closed, &Closed { amount, effective_at_ledger });
        env.events().publish_event(&ClosedEvent { amount });
    }

    /// Withdraw the committed amount to `to` after the channel is closed.
    /// Called by anyone.
    ///
    /// # Auth
    /// None.
    pub fn withdraw(env: Env) -> Result<(), Error> {
        let closed: Closed = env.storage().instance().get(&DataKey::Closed).ok_or(Error::NotClosed)?;
        if env.ledger().sequence() < closed.effective_at_ledger {
            return Err(Error::ClosePeriodNotElapsed);
        }
        if closed.amount == 0 {
            return Err(Error::NotClosed);
        }
        let to: Address = env.storage().instance().get(&DataKey::To).unwrap();
        env.storage().instance().set(
            &DataKey::Closed,
            &Closed {
                amount: 0,
                effective_at_ledger: closed.effective_at_ledger,
            },
        );
        Self::token_client(&env).transfer(&env.current_contract_address(), &to, &closed.amount);
        env.events().publish_event(&WithdrawEvent { to, amount: closed.amount });
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
        let closed: Closed = env.storage().instance().get(&DataKey::Closed).ok_or(Error::NotClosed)?;
        if env.ledger().sequence() < closed.effective_at_ledger {
            return Err(Error::ClosePeriodNotElapsed);
        }
        let from: Address = env.storage().instance().get(&DataKey::From).unwrap();
        from.require_auth();
        let tc = Self::token_client(&env);
        let balance = tc.balance(&env.current_contract_address());
        let refundable = balance - closed.amount;
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
