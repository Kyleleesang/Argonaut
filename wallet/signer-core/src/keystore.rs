//! The `.falcon.json` keystore: the same on-disk format `argonaut-node
//! falcon-key generate` has always written — browser-generated and
//! CLI-generated keystores are interchangeable.

use crate::{account_id, ss58::ss58_address, Keypair};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use rand_core::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};

pub const KDF_CONTEXT: &str = "argonaut-falcon-keystore-v1";

#[derive(Clone, Serialize, Deserialize)]
pub struct FalconKeystoreFile {
    /// SS58-encoded AccountId (`blake3(public_key)`).
    pub address: String,
    #[serde(with = "hex::serde")]
    pub public_key: Vec<u8>,
    /// `true` if `secret_key` is ChaCha20-Poly1305-encrypted (see `kdf_salt`/`nonce`),
    /// `false` if it's the raw signing key.
    pub encrypted: bool,
    #[serde(with = "hex::serde")]
    pub secret_key: Vec<u8>,
    #[serde(with = "hex::serde")]
    pub kdf_salt: Vec<u8>,
    #[serde(with = "hex::serde")]
    pub nonce: Vec<u8>,
}

fn derive_key(kdf_salt: &[u8], passphrase: &str) -> [u8; 32] {
    let mut key_material = kdf_salt.to_vec();
    key_material.extend_from_slice(passphrase.as_bytes());
    blake3::derive_key(KDF_CONTEXT, &key_material)
}

/// Seal a keypair into a keystore file. With a passphrase the signing key is
/// encrypted at rest (ChaCha20-Poly1305, key derived via `blake3::derive_key`);
/// without one it is stored in the clear — callers decide whether to warn.
pub fn seal(
    keypair: &Keypair,
    passphrase: Option<&str>,
    rng: &mut (impl RngCore + CryptoRng),
) -> Result<FalconKeystoreFile, String> {
    let address = ss58_address(&account_id(&keypair.verifying_key));

    let (secret_key, kdf_salt, nonce, encrypted) = match passphrase {
        Some(passphrase) => {
            let mut salt = [0u8; 16];
            rng.fill_bytes(&mut salt);
            let mut nonce_bytes = [0u8; 12];
            rng.fill_bytes(&mut nonce_bytes);

            let derived_key = derive_key(&salt, passphrase);
            let cipher = ChaCha20Poly1305::new((&derived_key).into());
            let ciphertext = cipher
                .encrypt(Nonce::from_slice(&nonce_bytes), keypair.signing_key.as_ref())
                .map_err(|_| "failed to encrypt signing key".to_string())?;

            (ciphertext, salt.to_vec(), nonce_bytes.to_vec(), true)
        },
        None => (keypair.signing_key.clone(), Vec::new(), Vec::new(), false),
    };

    Ok(FalconKeystoreFile {
        address,
        public_key: keypair.verifying_key.clone(),
        encrypted,
        secret_key,
        kdf_salt,
        nonce,
    })
}

impl FalconKeystoreFile {
    /// Recover the raw signing key, decrypting with `passphrase` if the file
    /// is encrypted.
    pub fn open(&self, passphrase: Option<&str>) -> Result<Vec<u8>, String> {
        if !self.encrypted {
            return Ok(self.secret_key.clone());
        }
        let passphrase =
            passphrase.ok_or_else(|| "keystore is encrypted; --passphrase is required".to_string())?;
        let derived_key = derive_key(&self.kdf_salt, passphrase);
        let cipher = ChaCha20Poly1305::new((&derived_key).into());
        cipher
            .decrypt(Nonce::from_slice(&self.nonce), self.secret_key.as_ref())
            .map_err(|_| "failed to decrypt signing key: wrong passphrase?".to_string())
    }
}
