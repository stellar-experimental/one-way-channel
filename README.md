# Channel

A unidirectional payment channel contract for Soroban (Stellar).

A funder (`from`) deposits tokens into a channel contract destined for a
recipient (`to`). The funder issues off-chain signed commitments for increasing
amounts. The recipient can close the channel at any time to claim the
authorized amount, and the funder can reclaim the remainder.

## How it works

1. **Open** -- Deploy the contract with the token, funder, recipient, commitment
   key, initial deposit, and abort ledger count.
2. **Off-chain** -- The funder signs commitments (using `prepare_commitment` to
   get the payload) for increasing amounts and sends them to the recipient.
3. **Close** -- The recipient closes the channel with a commitment. This is the
   typical way to close a channel.
4. **Abort start/finish** -- If the recipient doesn't close the channel, the
   funder can start an abort. After a waiting period anyone can finish the
   abort, resulting in a full refund. During the waiting period the recipient
   can dispute by calling `close` with a commitment.
5. **Withdraw** -- After close, anyone calls `withdraw` to transfer the closed
   amount to the recipient.
6. **Refund** -- The funder calls `refund` to reclaim the remainder.

## State diagram

```mermaid
stateDiagram-v2
    [*] --> Open: __constructor

    Open --> Open: top_up
    Open --> Aborting: abort_start
    Open --> Closed: close(commitment)

    Aborting --> Closed: abort_finish [after abort_at_ledger]
    Aborting --> Closed: close(commitment) [dispute]

    Closed --> Withdrawn: withdraw
    Closed --> Closed: refund [remainder only]

    Withdrawn --> [*]: refund [remainder]
```

## Functions

| Function | Description | Who can call | Auth required |
|---|---|---|---|
| `__constructor` | Deploy the contract with the token, funder, recipient, commitment key, initial deposit, and abort ledger count. | Deployer | `from` |
| `top_up` | Top up the channel with the stored token from the stored from address. | Anyone | `from` |
| `prepare_commitment` | Returns the commitment payload that needs to be signed by the commitment_key. | Anyone | None |
| `balance_deposited` | Returns the total amount deposited in the channel. | Anyone | None |
| `abort_start` | Start aborting the channel. If undisputed, results in a full refund to the funder. | Funder | `from` |
| `abort_finish` | Finish the abort after the abort_at_ledger has been reached. Closes with amount 0, resulting in a full refund. | Anyone | None |
| `close` | Close the channel by submitting a commitment. No waiting period. | Recipient | `to` + commitment sig |
| `withdraw` | Withdraw the authorized amount to `to` after the channel is closed. | Anyone | None |
| `refund` | Refund the funder's portion of the balance. Can be called after the channel is closed. | Funder | `from` |

## Commitment format

The commitment is a `Commitment` struct serialized to XDR (ScVal Map):

| Field | Type | Value |
|---|---|---|
| `prefix` | Symbol | `chancmmt` |
| `channel` | Address | Channel contract address |
| `amount` | i128 | Authorized amount |

The funder signs the XDR bytes with their ed25519 key
(`commitment_key`). The signature never expires.
