//! Minimal SS58 encoding (the subset needed for 32-byte AccountIds), so the
//! browser signer doesn't pull `sp-core` into its wasm build. Byte-compat with
//! `sp_core::crypto::Ss58Codec` is pinned in this crate's tests.

use blake2::{Blake2b512, Digest};

/// The prefix `AccountId32::to_ss58check()` uses by default (Substrate
/// generic = 42) — what `falcon-key generate` has always written.
pub const SS58_PREFIX: u16 = 42;

pub fn ss58_address(account: &[u8; 32]) -> String {
    ss58_address_with_prefix(account, SS58_PREFIX)
}

pub fn ss58_address_with_prefix(account: &[u8; 32], prefix: u16) -> String {
    let ident = prefix & 0b0011_1111_1111_1111;
    let mut data = Vec::with_capacity(37);
    match ident {
        0..=63 => data.push(ident as u8),
        _ => {
            let first = (((ident & 0b0000_0000_1111_1100) >> 2) as u8) | 0b0100_0000;
            let second = ((ident >> 8) as u8) | (((ident & 0b0000_0000_0000_0011) as u8) << 6);
            data.push(first);
            data.push(second);
        },
    }
    data.extend_from_slice(account);

    let mut hasher = Blake2b512::new();
    hasher.update(b"SS58PRE");
    hasher.update(&data);
    let checksum = hasher.finalize();
    data.extend_from_slice(&checksum[..2]);

    bs58::encode(data).into_string()
}
