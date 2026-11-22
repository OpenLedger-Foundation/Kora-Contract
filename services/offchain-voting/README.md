# Off-chain ballot encryption and key rotation

`EncryptedMemoryVoteStore` demonstrates the ballot envelope used by vote-store
adapters: AES-256-GCM ciphertext, a random 96-bit nonce, an authentication tag,
and a key ID. The proposal/voter pair is authenticated as associated data. Do
not persist plaintext vote choices or signatures in a production adapter.

## Rotation procedure

1. Back up the ballot store and verify a restorable copy before changing keys.
2. Provision a new 32-byte key as a new version of
   `/kora/<environment>/offchain-voting`; keep the current key available to the
   workload during the overlap.
3. Roll out the new key ID and key. Call `rotateKey(newKey)` in the vote-store
   adapter: it accepts the old key while re-encrypting existing ballots under
   the new key. Poll mode reservations and open proposal IDs are unchanged.
4. Verify open polls can read existing ballots, accept new votes, and finalize.
   Keep both key versions enabled until every replica and durable backup has
   completed re-encryption.
5. Call `finishRotation()` only after that verification. Remove the old key
   from the secret and revoke its secret version according to retention policy.
6. If verification fails, restore the previous secret and key-ring configuration
   before removing either key. Do not close or recreate polls as a rotation
   workaround.

Production stores must make re-encryption durable and atomic, and must retain
both key IDs during rolling deployments. `EncryptedMemoryVoteStore` is intended
for tests/local use; it does not persist ballots across process restarts.
