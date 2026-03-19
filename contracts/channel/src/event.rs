use soroban_sdk::{contractevent, Address, BytesN};

/// Emitted when the channel is opened via the constructor.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Open {
    /// The funder who deposited tokens into the channel.
    pub from: Address,
    /// The ed25519 public key used to verify commitment signatures.
    pub commitment_key: BytesN<32>,
    /// The recipient who can close the channel using a commitment.
    pub to: Address,
    /// The SEP-41 token used for payments.
    pub token: Address,
    /// The initial deposit amount.
    pub amount: i128,
    /// The number of ledgers the funder has to wait before refund after close_start.
    pub refund_waiting_period: u32,
}

/// Emitted when the channel close becomes effective, either immediately via
/// close or after a waiting period via close_start.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Close {
    /// The ledger sequence number at which the close becomes effective and
    /// the funder can call refund.
    pub effective_at_ledger: u32,
}

/// Emitted when the recipient receives funds via close.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Withdraw {
    /// The recipient who received the funds.
    pub to: Address,
    /// The amount transferred to the recipient.
    pub amount: i128,
}

/// Emitted when the funder reclaims remaining funds via refund.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Refund {
    /// The funder who received the refund.
    pub from: Address,
    /// The amount transferred to the funder. This is the entire remaining
    /// balance of the channel at the time of the refund.
    pub amount: i128,
}
