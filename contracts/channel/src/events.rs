use soroban_sdk::{contractevent, Address, BytesN};

/// Emitted when the channel is opened via the constructor.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenEvent {
    pub from: Address,
    pub commitment_key: BytesN<32>,
    pub to: Address,
    pub token: Address,
    pub amount: i128,
    pub close_ledger_count: u32,
}

/// Emitted when the channel is closed via close.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloseEvent {
    pub effective_at_ledger: u32,
}

/// Emitted when the recipient withdraws via withdraw.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WithdrawEvent {
    pub to: Address,
    pub amount: i128,
}

/// Emitted when the funder reclaims remaining funds via refund.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundEvent {
    pub from: Address,
    pub amount: i128,
}
