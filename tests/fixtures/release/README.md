`SHA256SUMS` signed with the real release key by `scripts/release-sign.sh`,
the way the release workflow signs. `src/updates/signature.rs` checks it
against the public key built into Sideporch.
