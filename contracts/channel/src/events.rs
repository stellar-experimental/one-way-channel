use soroban_sdk::{contractevent, Address, BytesN};

/// Emitted when the channel is opened via the constructor.
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

/// Emitted when a close is started via close_start. The close_at_ledger is the
/// ledger at which close_finish can be called.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloseStartEvent {
    pub close_at_ledger: u32,
}

/// Emitted when the channel is closed via close_finish or close_immediately.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosedEvent {
    pub amount: i128,
}

/// Emitted when the closed amount is withdrawn to the recipient.
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
