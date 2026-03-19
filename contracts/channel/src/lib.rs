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
    Withdrawn,
    Closed,
}

#[contracttype]
pub struct Commitment {
    domain: Symbol,
    channel: Address,
    amount: i128,
}

impl Commitment {
    pub fn new(channel: Address, amount: i128) -> Self {
        Commitment {
            domain: symbol_short!("chancmmt"),
            channel,
            amount,
        }
    }

    fn into_bytes(self) -> Bytes {
        let env = self.channel.env().clone();
        self.to_xdr(&env)
    }

    fn verify(self, sig: &BytesN<64>) {
        let env = self.channel.env().clone();
        let commitment_key: BytesN<32> = env.storage().instance().get(&DataKey::CommitmentKey).unwrap();
        let payload = self.into_bytes();
        env.crypto().ed25519_verify(&commitment_key, &payload, sig);
    }
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

    /// Returns the balance of the channel. This is the deposited amount minus
    /// any amount already withdrawn.
    /// Called by anyone.
    ///
    /// # Auth
    /// None.
    pub fn balance(env: Env) -> i128 {
        Self::token_client(&env).balance(&env.current_contract_address())
    }

    /// Returns the commitment payload that needs to be signed by the
    /// commitment_key. The signed commitment can be passed to withdraw.
    /// Called by anyone.
    ///
    /// # Auth
    /// None.
    pub fn prepare_commitment(env: Env, amount: i128) -> Bytes {
        Commitment::new(env.current_contract_address(), amount).into_bytes(&env)
    }

    /// Withdraw the committed amount to the recipient. The commitment amount is
    /// the total amount, but only the difference from what has already been withdrawn is
    /// transferred. Can be called at any time.
    /// Called by the recipient (to).
    ///
    /// # Auth
    /// - `to`: required.
    /// - Commitment signature serves as commitment_key authorization.
    pub fn withdraw(env: Env, amount: i128, sig: BytesN<64>) {
        let to: Address = env.storage().instance().get(&DataKey::To).unwrap();
        to.require_auth();
        Commitment::new(env.current_contract_address(), amount).verify(&env, &sig);
        let withdrawn: i128 = env.storage().instance().get(&DataKey::Withdrawn).unwrap_or(0);
        let payout = amount - withdrawn;
        if payout > 0 {
            env.storage().instance().set(&DataKey::Withdrawn, &amount);
            Self::token_client(&env).transfer(&env.current_contract_address(), &to, &payout);
            env.events().publish_event(&WithdrawEvent { to, amount: payout });
        }
    }

    /// Close the channel, effective after a waiting period. The recipient can
    /// still withdraw during the waiting period. After the close is effective,
    /// the funder can call refund to reclaim the remaining balance.
    /// Called by the funder (from).
    ///
    /// # Auth
    /// - `from`: required.
    pub fn close(env: Env) {
        let from: Address = env.storage().instance().get(&DataKey::From).unwrap();
        from.require_auth();
        let close_ledger_count: u32 = env.storage().instance().get(&DataKey::CloseLedgerCount).unwrap();
        let effective_at_ledger = env.ledger().sequence() + close_ledger_count;
        env.storage().instance().set(&DataKey::Closed, &effective_at_ledger);
        env.events().publish_event(&CloseEvent { effective_at_ledger });
    }

    /// Refund the remaining balance to the funder after the close is effective.
    /// Called by the funder (from).
    ///
    /// # Auth
    /// - `from`: required.
    pub fn refund(env: Env) -> Result<(), Error> {
        let effective_at_ledger: u32 = env.storage().instance().get(&DataKey::Closed).ok_or(Error::NotClosed)?;
        if env.ledger().sequence() < effective_at_ledger {
            return Err(Error::ClosePeriodNotElapsed);
        }
        let from: Address = env.storage().instance().get(&DataKey::From).unwrap();
        from.require_auth();
        let tc = Self::token_client(&env);
        let balance = tc.balance(&env.current_contract_address());
        if balance > 0 {
            tc.transfer(&env.current_contract_address(), &from, &balance);
            env.events().publish_event(&RefundEvent { from, amount: balance });
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
