# Channel

A unidirectional payment channel contract for Soroban (Stellar).

A funder (`from`) deposits tokens into a channel contract destined for a
recipient (`to`). The funder issues off-chain signed commitments for increasing
amounts. The recipient can settle at any time to claim the committed amount,
and the funder can close the channel to reclaim the remainder.

## How it works

1. **Open** -- Deploy the contract with the token, funder, recipient, commitment
   key, initial deposit, and close ledger count.
2. **Off-chain** -- The funder signs commitments (using `prepare_commitment` to
   get the payload) for increasing amounts and sends them to the recipient.
3. **Settle** -- The recipient settles at any time with a commitment, receiving
   the committed amount. The commitment amount is the total amount, so only the
   difference from previous settlements is transferred.
4. **Close** -- The funder closes the channel. The close becomes effective after
   a waiting period. During the waiting period the recipient can still settle.
5. **Refund** -- After the close is effective, the funder calls `refund` to
   reclaim the remaining balance.

## State diagram

```mermaid
stateDiagram-v2
    [*] --> Open: __constructor

    Open --> Open: top_up
    Open --> Open: settle
    Open --> Closing: close

    Closing --> Closing: settle
    Closing --> Closed: [after wait]

    Closed --> [*]: refund
```

## Functions

| Function | Description | Who can call | Auth required |
|---|---|---|---|
| `__constructor` | Deploy the contract with the token, funder, recipient, commitment key, initial deposit, and close ledger count. | Deployer | `from` |
| `top_up` | Top up the channel with the stored token from the stored from address. | Anyone | `from` |
| `prepare_commitment` | Returns the commitment payload that needs to be signed by the commitment_key. | Anyone | None |
| `balance_deposited` | Returns the total amount deposited in the channel. | Anyone | None |
| `settle` | Settle the committed amount to the recipient. The commitment amount is the total amount, only the difference from previous settlements is transferred. Can be called at any time. | Recipient | `to` + commitment sig |
| `close` | Close the channel, effective after a waiting period. The recipient can still settle during the wait. | Funder | `from` |
| `refund` | Refund the remaining balance to the funder after the close is effective. | Funder | `from` |

## Commitment format

The commitment is a `Commitment` struct serialized to XDR (ScVal Map):

| Field | Type | Value |
|---|---|---|
| `prefix` | Symbol | `chancmmt` |
| `channel` | Address | Channel contract address |
| `amount` | i128 | Committed amount |

The funder signs the XDR bytes with their ed25519 key
(`commitment_key`). The signature never expires.
