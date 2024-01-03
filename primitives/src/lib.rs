use pqcrypto_falcon::falcon512::{self, PublicKey, SecretKey, SignedMessage, DetachedSignature};
use pqcrypto_traits::sign::{VerificationError, PublicKey as pubkey};
use sp_core::crypto::{AccountId32, Ss58AddressFormat, Ss58Codec};


pub struct FalconHostFunctions;

impl FalconHostFunctions {

    pub fn generate_keypair() -> (PublicKey, SecretKey) {
        let (pk, sk) = falcon512::keypair();
        (pk, sk)
    }

    pub fn sign(message: &[u8], sk: &SecretKey ) -> SignedMessage {
        falcon512::sign(message, sk)
    }

    pub fn verify(signature: &DetachedSignature, message: &[u8], public_key: &PublicKey) -> Result<(), VerificationError> {
        falcon512::verify_detached_signature(signature, message, public_key)
    }

    pub fn open(signature: &SignedMessage, public_key: &PublicKey) -> Result<Vec<u8>, VerificationError> {
        falcon512::open(signature, public_key)
    }

    pub fn detached_signature(message: &[u8], secret_key: &SecretKey) -> DetachedSignature {
        falcon512::detached_sign(message, secret_key)
    }

    pub fn hash_public_key(public_key: &PublicKey) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(public_key.as_bytes());
        hasher.finalize().into()
    }
    
    pub fn generate_address(hashed_key: [u8; 32]) -> String {
        let address = sp_core::crypto::AccountId32::from(hashed_key).to_ss58check_with_version(
            sp_core::crypto::Ss58AddressFormat::custom(42));
        address
    }
    

}

#[cfg(test)]
mod falcon_tests {
    use super::*;
    #[test]
    fn sign_and_open() {
        let (pk, sk) = FalconHostFunctions::generate_keypair();
        let message = b"Hello, world!";
        let signature = FalconHostFunctions::sign(message, &sk);
        let test_signature = FalconHostFunctions::open(&signature, &pk);
        assert_eq!(test_signature.unwrap(), message);
    }

    #[test]
    fn hash_public_key() {
        let (pk, _) = FalconHostFunctions::generate_keypair();
        let hashed_key = FalconHostFunctions::hash_public_key(&pk);
        assert_eq!(hashed_key.len(), 32);
    }


    #[test]
    fn sign_and_verify() {
        let (pk, sk) = FalconHostFunctions::generate_keypair();
        let message = b"Hello, world!";
        let signature = FalconHostFunctions::detached_signature(message, &sk);
        assert!(falcon512::verify_detached_signature(&signature, message, &pk).is_ok());
    }

    #[test]
    fn generate_address() {
        let (pk, _) = FalconHostFunctions::generate_keypair();
        let hashed_key = FalconHostFunctions::hash_public_key(&pk);
        let address = FalconHostFunctions::generate_address(hashed_key);
        println!("The substrate address generated is: {}", address);
        assert_eq!(address.len(), 48);
    }
}