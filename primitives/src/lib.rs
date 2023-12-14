use pqcrypto_falcon::falcon512::{self, PublicKey, SecretKey, SignedMessage, DetachedSignature};
use pqcrypto_traits::sign::VerificationError;


pub struct FalconHostFunctions;

impl FalconHostFunctions {

    pub fn falcon512_keypair() -> (PublicKey, SecretKey) {
        let (pk, sk) = falcon512::keypair();
        (pk, sk)
    }

    pub fn falcon512_sign(message: &[u8], sk: &SecretKey ) -> SignedMessage {
        falcon512::sign(message, sk)
    }


    pub fn falcon512_verify(signature: &DetachedSignature, message: &[u8], public_key: &PublicKey) -> Result<(), VerificationError> {
        falcon512::verify_detached_signature(signature, message, public_key)
    }

    pub fn falcon512_open(signature: &SignedMessage, public_key: &PublicKey) -> Result<Vec<u8>, VerificationError> {
        falcon512::open(signature, public_key)
    }

    pub fn falcon512_detached_signature(message: &[u8], secret_key: &SecretKey) -> DetachedSignature {
        falcon512::detached_sign(message, secret_key)
    }

}