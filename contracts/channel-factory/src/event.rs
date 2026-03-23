use soroban_sdk::Address;

/// Emitted when a new channel is opened via the factory.
#[soroban_sdk::contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Open {
    /// The address of the deployed channel contract.
    pub channel: Address,
}
