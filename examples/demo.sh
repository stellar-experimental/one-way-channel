#!/bin/bash
set -euo pipefail

# Demo: Open a channel, make off-chain payments, recipient closes, funder
# starts close and refunds.
#
# Prerequisites:
# - stellar-cli installed
# - ed25519 installed (make install-tool-ed25519)
# - channel contract built (make build)

WASM="target/wasm32v1-none/release/channel.wasm"

echo "=== Building contract ==="
stellar contract build

echo "=== Setting up network ==="
stellar network use testnet

echo "=== Generating and funding identities ==="
stellar keys generate funder --fund 2>/dev/null || true
stellar keys generate recipient --fund 2>/dev/null || true
echo "Funder:    $(stellar keys address funder)"
echo "Recipient: $(stellar keys address recipient)"

echo ""
echo "=== Generating commitment key ==="
COMMITMENT_SKEY=$(ed25519 gen)
COMMITMENT_PKEY=$(ed25519 pub $COMMITMENT_SKEY)
echo "Commitment secret key: $COMMITMENT_SKEY"
echo "Commitment public key: $COMMITMENT_PKEY"

echo ""
echo "=== Deploying native asset contract ==="
stellar contract alias add native --id $(stellar contract id asset --asset native)
echo "Token: $(stellar contract id asset --asset native)"

echo ""
echo "=== Installing channel wasm ==="
WASM_HASH=$(stellar contract upload \
    --wasm $WASM \
    --source funder)
echo "Wasm hash: $WASM_HASH"

echo ""
echo "========================================="
echo "  Scenario 1: Recipient closes channel"
echo "========================================="

echo ""
echo "=== Deploying channel contract ==="
stellar contract deploy \
    --alias channel1 \
    --wasm-hash $WASM_HASH \
    --source funder \
    -- \
    --token native \
    --from funder \
    --commitment_key $COMMITMENT_PKEY \
    --to recipient \
    --amount 10000000 \
    --refund_waiting_period 5
echo "Channel: $(stellar contract alias show channel1)"

echo ""
echo "=== Channel state after open ==="
echo -n "  Deposited: " && stellar contract invoke --id channel1 --send=no -- deposited
echo -n "  Withdrawn: " && stellar contract invoke --id channel1 --send=no -- withdrawn
echo -n "  Balance:   " && stellar contract invoke --id channel1 --send=no -- balance

echo ""
echo "=== Off-chain: Funder signs commitment ==="
COMMITMENT_1=$(stellar contract invoke --id channel1 --send=no -- prepare_commitment --amount 6000000)
SIG_1=$(ed25519 sign $COMMITMENT_SKEY $COMMITMENT_1)
echo "  Payment: 6,000,000 stroops, sig=$SIG_1"

echo ""
echo "=== Recipient closes channel using commitment ==="
stellar keys use recipient
stellar contract invoke \
    --id channel1 \
    -- close --amount 6000000 --sig $SIG_1

echo ""
echo "=== Channel state after close ==="
echo -n "  Deposited: " && stellar contract invoke --id channel1 --send=no -- deposited
echo -n "  Withdrawn: " && stellar contract invoke --id channel1 --send=no -- withdrawn
echo -n "  Balance:   " && stellar contract invoke --id channel1 --send=no -- balance

echo ""
echo "========================================="
echo "  Scenario 2: Funder closes via close_start + refund"
echo "========================================="

echo ""
echo "=== Deploying channel contract ==="
stellar contract deploy \
    --alias channel2 \
    --wasm-hash $WASM_HASH \
    --source funder \
    -- \
    --token native \
    --from funder \
    --commitment_key $COMMITMENT_PKEY \
    --to recipient \
    --amount 10000000 \
    --refund_waiting_period 5
echo "Channel: $(stellar contract alias show channel2)"

echo ""
echo "=== Channel state after open ==="
echo -n "  Deposited: " && stellar contract invoke --id channel2 --send=no -- deposited
echo -n "  Withdrawn: " && stellar contract invoke --id channel2 --send=no -- withdrawn
echo -n "  Balance:   " && stellar contract invoke --id channel2 --send=no -- balance

echo ""
echo "=== Funder starts closing channel ==="
stellar keys use funder
stellar contract invoke \
    --id channel2 \
    -- close_start

echo ""
echo "=== Funder refunds remainder (retrying until close is effective) ==="
until stellar contract invoke --id channel2 -- refund 2>/dev/null; do
    echo "  Close not yet effective, retrying in 5s..."
    sleep 5
done

echo ""
echo "=== Channel state after refund ==="
echo -n "  Deposited: " && stellar contract invoke --id channel2 --send=no -- deposited
echo -n "  Withdrawn: " && stellar contract invoke --id channel2 --send=no -- withdrawn
echo -n "  Balance:   " && stellar contract invoke --id channel2 --send=no -- balance

echo ""
echo "========================================="
echo "  Scenario 3: Funder close_start, recipient closes with commitment"
echo "========================================="

echo ""
echo "=== Deploying channel contract ==="
stellar contract deploy \
    --alias channel3 \
    --wasm-hash $WASM_HASH \
    --source funder \
    -- \
    --token native \
    --from funder \
    --commitment_key $COMMITMENT_PKEY \
    --to recipient \
    --amount 10000000 \
    --refund_waiting_period 5
echo "Channel: $(stellar contract alias show channel3)"

echo ""
echo "=== Channel state after open ==="
echo -n "  Deposited: " && stellar contract invoke --id channel3 --send=no -- deposited
echo -n "  Withdrawn: " && stellar contract invoke --id channel3 --send=no -- withdrawn
echo -n "  Balance:   " && stellar contract invoke --id channel3 --send=no -- balance

echo ""
echo "=== Off-chain: Funder signs commitment ==="
COMMITMENT_3=$(stellar contract invoke --id channel3 --send=no -- prepare_commitment --amount 6000000)
SIG_3=$(ed25519 sign $COMMITMENT_SKEY $COMMITMENT_3)
echo "  Payment: 6,000,000 stroops, sig=$SIG_3"

echo ""
echo "=== Funder starts closing channel ==="
stellar keys use funder
stellar contract invoke \
    --id channel3 \
    -- close_start

echo ""
echo "=== Recipient closes channel during waiting period using commitment ==="
stellar keys use recipient
stellar contract invoke \
    --id channel3 \
    -- close --amount 6000000 --sig $SIG_3

echo ""
echo "=== Channel state after close ==="
echo -n "  Deposited: " && stellar contract invoke --id channel3 --send=no -- deposited
echo -n "  Withdrawn: " && stellar contract invoke --id channel3 --send=no -- withdrawn
echo -n "  Balance:   " && stellar contract invoke --id channel3 --send=no -- balance

echo ""
echo "========================================="
echo "  Scenario 4: Settle before close"
echo "========================================="

echo ""
echo "=== Deploying channel contract ==="
stellar contract deploy \
    --alias channel4 \
    --wasm-hash $WASM_HASH \
    --source funder \
    -- \
    --token native \
    --from funder \
    --commitment_key $COMMITMENT_PKEY \
    --to recipient \
    --amount 10000000 \
    --refund_waiting_period 5
echo "Channel: $(stellar contract alias show channel4)"

echo ""
echo "=== Channel state after open ==="
echo -n "  Deposited: " && stellar contract invoke --id channel4 --send=no -- deposited
echo -n "  Withdrawn: " && stellar contract invoke --id channel4 --send=no -- withdrawn
echo -n "  Balance:   " && stellar contract invoke --id channel4 --send=no -- balance

echo ""
echo "=== Off-chain: Funder signs commitments ==="
COMMITMENT_4A=$(stellar contract invoke --id channel4 --send=no -- prepare_commitment --amount 3000000)
SIG_4A=$(ed25519 sign $COMMITMENT_SKEY $COMMITMENT_4A)
echo "  Payment 1: cumulative 3,000,000 stroops, sig=$SIG_4A"

COMMITMENT_4B=$(stellar contract invoke --id channel4 --send=no -- prepare_commitment --amount 6000000)
SIG_4B=$(ed25519 sign $COMMITMENT_SKEY $COMMITMENT_4B)
echo "  Payment 2: cumulative 6,000,000 stroops, sig=$SIG_4B"

COMMITMENT_4C=$(stellar contract invoke --id channel4 --send=no -- prepare_commitment --amount 8000000)
SIG_4C=$(ed25519 sign $COMMITMENT_SKEY $COMMITMENT_4C)
echo "  Payment 3: cumulative 8,000,000 stroops, sig=$SIG_4C"

echo ""
echo "=== Recipient settles using commitment 1 ==="
stellar keys use recipient
stellar contract invoke \
    --id channel4 \
    -- settle --amount 3000000 --sig $SIG_4A

echo ""
echo "=== Channel state after first settle ==="
echo -n "  Deposited: " && stellar contract invoke --id channel4 --send=no -- deposited
echo -n "  Withdrawn: " && stellar contract invoke --id channel4 --send=no -- withdrawn
echo -n "  Balance:   " && stellar contract invoke --id channel4 --send=no -- balance

echo ""
echo "=== Recipient settles using commitment 2 (skipping none) ==="
stellar contract invoke \
    --id channel4 \
    -- settle --amount 6000000 --sig $SIG_4B

echo ""
echo "=== Channel state after second settle ==="
echo -n "  Deposited: " && stellar contract invoke --id channel4 --send=no -- deposited
echo -n "  Withdrawn: " && stellar contract invoke --id channel4 --send=no -- withdrawn
echo -n "  Balance:   " && stellar contract invoke --id channel4 --send=no -- balance

echo ""
echo "=== Recipient closes channel using commitment 3 ==="
stellar contract invoke \
    --id channel4 \
    -- close --amount 8000000 --sig $SIG_4C

echo ""
echo "=== Channel state after close ==="
echo -n "  Deposited: " && stellar contract invoke --id channel4 --send=no -- deposited
echo -n "  Withdrawn: " && stellar contract invoke --id channel4 --send=no -- withdrawn
echo -n "  Balance:   " && stellar contract invoke --id channel4 --send=no -- balance

echo ""
echo "=== Done ==="
echo ""
echo "Summary:"
echo "  Channel 1 (recipient closes):             $(stellar contract alias show channel1)"
echo "  Channel 2 (funder close_start + refund):   $(stellar contract alias show channel2)"
echo "  Channel 3 (close_start, recipient closes): $(stellar contract alias show channel3)"
echo "  Channel 4 (settle before close):           $(stellar contract alias show channel4)"
