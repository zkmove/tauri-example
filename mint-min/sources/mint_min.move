/// Minimal confidential-asset mint: a token is minted only after the
/// zk proof passes on-chain verification. The proof is uploaded in
/// chunks beforehand (Sui pure-argument size limit) and consumed here
/// as an `ArtifactBuilder`, reusing the verifier API's tested
/// `verify_proof_from_builder` flow.
///
/// v2 (strong binding): the public inputs are constructed on-chain from
/// `encrypted_amount`, so the minted amount is cryptographically bound
/// to the proof — a proof for a different amount cannot mint this one.
module mint_min::mint_min;

use verifier_api::artifact_builder::{Self, ArtifactBuilder};
use verifier_api::native_verifier::SerializedVK;
use verifier_api::serialized_params_store::SerializedParams;
use verifier_api::serialized_public_inputs;

const EInvalidProof: u64 = 1;
const EZeroAmount: u64 = 2;

public struct Store has key {
    id: UID,
    balance: u256,
}

entry fun register(ctx: &mut TxContext) {
    transfer::transfer(Store { id: object::new(ctx), balance: 0 }, ctx.sender())
}

/// Verify-then-mint. The proof must bind `encrypted_amount` as its
/// public input (prover side: `--pubs-indices` selecting the
/// encrypted-value argument of the circuit entry function).
entry fun mint(
    store: &mut Store,
    params: &SerializedParams,
    vk: &SerializedVK,
    proof_builder: ArtifactBuilder,
    expected_proof_digest: vector<u8>,
    kzg_variant: u8,
    k_present: bool,
    k: u32,
    encrypted_amount: u256,
) {
    mint_from_builder(
        store,
        params,
        vk,
        proof_builder,
        expected_proof_digest,
        kzg_variant,
        k_present,
        k,
        encrypted_amount,
    )
}

public fun mint_from_builder(
    store: &mut Store,
    params: &SerializedParams,
    vk: &SerializedVK,
    proof_builder: ArtifactBuilder,
    expected_proof_digest: vector<u8>,
    kzg_variant: u8,
    k_present: bool,
    k: u32,
    encrypted_amount: u256,
) {
    assert!(encrypted_amount > 0, EZeroAmount);

    let mut public_inputs = serialized_public_inputs::empty(
        serialized_public_inputs::get_vm_public_inputs_column_count(),
    );
    serialized_public_inputs::push_u256(&mut public_inputs, encrypted_amount);

    assert!(
        artifact_builder::verify_proof_from_builder(
            params,
            vk,
            proof_builder,
            expected_proof_digest,
            serialized_public_inputs::to_bcs_bytes(&public_inputs),
            kzg_variant,
            k_present,
            k,
        ),
        EInvalidProof,
    );
    store.balance = store.balance + encrypted_amount;
}

public fun balance(store: &Store): u256 {
    store.balance
}
