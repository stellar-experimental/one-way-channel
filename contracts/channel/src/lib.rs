#![no_std]
use soroban_sdk::{contract, contracterror, contractevent, contractimpl, contracttype, symbol_short, token, xdr::ToXdr, Address, Bytes, BytesN, Env, Symbol};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotClosing = 1,
    ClosePeriodNotElapsed = 2,
    NotClosed = 3,
    NotRefundable = 4,
}

#[contracttype]
pub enum DataKey {
    Token,
    From,
    FromVoucherAuthKey,
    To,
    CloseLedgerCount,
    Close,
    Closed,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenEvent {
    pub from: Address,
    pub from_voucher_auth_key: BytesN<32>,
    pub to: Address,
    pub token: Address,
    pub amount: i128,
    pub close_ledger_count: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloseStartEvent {
    pub amount: i128,
    pub close_at_ledger: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosedEvent {
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WithdrawEvent {
    pub to: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundEvent {
    pub from: Address,
    pub amount: i128,
}

#[contracttype]
pub struct Voucher {
    pub prefix: Symbol,
    pub channel: Address,
    pub token: Address,
    pub amount: i128,
}

impl Voucher {
    fn new(env: &Env, amount: i128) -> Self {
        let token: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        Voucher {
            prefix: symbol_short!("chanvchr"),
            channel: env.current_contract_address(),
            token,
            amount,
        }
    }

    fn into_bytes(self, env: &Env) -> Bytes {
        self.to_xdr(env)
    }

    fn verify(self, env: &Env, sig: &BytesN<64>) {
        let from_voucher_auth_key: BytesN<32> = env.storage().instance().get(&DataKey::FromVoucherAuthKey).unwrap();
        let payload = self.into_bytes(env);
        env.crypto().ed25519_verify(&from_voucher_auth_key, &payload, sig);
    }
}

#[contracttype]
pub struct Close {
    pub amount: i128,
    pub close_at_ledger: u32,
}

#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
    pub fn __constructor(env: Env, token: Address, from: Address, from_voucher_auth_key: BytesN<32>, to: Address, amount: i128, close_ledger_count: u32) {
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::From, &from);
        env.storage().instance().set(&DataKey::FromVoucherAuthKey, &from_voucher_auth_key);
        env.storage().instance().set(&DataKey::To, &to);
        env.storage().instance().set(&DataKey::CloseLedgerCount, &close_ledger_count);
        Self::top_up(env.clone(), amount);
        env.events().publish_event(&OpenEvent {
            from,
            from_voucher_auth_key,
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

    /// Returns the voucher payload that needs to be signed by the from_voucher_auth_key.
    /// The signed voucher can be passed to close_start or close_immediately.
    /// Called by anyone.
    ///
    /// # Auth
    /// None.
    pub fn prepare_voucher(env: Env, amount: i128) -> Bytes {
        Voucher::new(&env, amount).into_bytes(&env)
    }

    /// Returns the total amount deposited in the channel.
    /// Called by anyone.
    ///
    /// # Auth
    /// None.
    pub fn balance_deposited(env: Env) -> i128 {
        Self::token_client(&env).balance(&env.current_contract_address())
    }

    /// Start closing the channel by submitting a voucher.
    /// Can be called again to overwrite a pending close.
    /// Called by anyone with a valid voucher.
    ///
    /// # Auth
    /// None. The voucher signature serves as authorization.
    pub fn close_start(env: Env, amount: i128, sig: BytesN<64>) {
        Voucher::new(&env, amount).verify(&env, &sig);
        let close_ledger_count: u32 = env.storage().instance().get(&DataKey::CloseLedgerCount).unwrap();
        let close_at_ledger = env.ledger().sequence() + close_ledger_count;
        env.storage().instance().set(&DataKey::Close, &Close { amount, close_at_ledger });
        env.events().publish_event(&CloseStartEvent { amount, close_at_ledger });
    }

    /// Finish the close after the close_at_ledger has been reached.
    /// Marks the channel as closed with the authorized amount.
    /// Called by anyone.
    ///
    /// Note: The recipient should prefer close_immediately since holding a voucher
    /// and being the recipient they can authorize an immediate exit without waiting.
    ///
    /// # Auth
    /// None.
    pub fn close_finish(env: Env) -> Result<(), Error> {
        let close: Close = env.storage().instance().get(&DataKey::Close).ok_or(Error::NotClosing)?;

        if env.ledger().sequence() < close.close_at_ledger {
            return Err(Error::ClosePeriodNotElapsed);
        }

        env.storage().instance().remove(&DataKey::Close);
        env.storage().instance().set(&DataKey::Closed, &close.amount);
        env.events().publish_event(&ClosedEvent { amount: close.amount });
        Ok(())
    }

    /// Close the channel immediately by submitting a voucher. No waiting period.
    /// Called by the recipient (to).
    ///
    /// # Auth
    /// - `to`: required.
    /// - Voucher signature serves as from_voucher_auth_key authorization.
    pub fn close_immediately(env: Env, amount: i128, sig: BytesN<64>) {
        let to: Address = env.storage().instance().get(&DataKey::To).unwrap();
        to.require_auth();
        Voucher::new(&env, amount).verify(&env, &sig);
        env.storage().instance().remove(&DataKey::Close);
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
        let to: Address = env.storage().instance().get(&DataKey::To).unwrap();
        env.storage().instance().remove(&DataKey::Closed);
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
        if env.storage().instance().has(&DataKey::Close) {
            return Err(Error::NotRefundable);
        }
        let closed_amount: i128 = env.storage().instance().get(&DataKey::Closed).unwrap_or(0);
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
