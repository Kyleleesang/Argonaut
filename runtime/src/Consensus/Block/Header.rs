use blake3 as Blake3;
use sp_core::H256;
//use the sp runtime header
use sp_runtime::traits::{Block as BlockT, Header, DigestItem as Digest};
use frame_support::dispatch::Parameter;
use blake3::*;

#[derive(Serialize, Deserialize, Clone, Copy, Hash, Parameter)]
pub struct BlockHeader {
	//Hash of the parent proposer Block
	pub parent: H256,
	//timestamp of the current Block
	pub timestamp: u128,
	//nonce for it
	pub nonce: u32,
	//merkle root of the content inside of a block
	pub contentRoot: H256,
	//vec of array of extra content, later check to see if you can just make it the extra content vec
	pub extraContent: [u8; 32],
	//Difficulty of the block
	pub difficulty: H256,
	//coinbase of the miner's address
	pub coinbase: AccountId
}

//implement headerT for BlockHeader
impl Header for BlockHeader {
	//type of the hash
	type Hash = blake3::Hash;
	//type of the hash
	type Hashing = Blake3;
	//type of the digest
	type Digest = Digest;
	//type of the number
	type Number = u128;
	//type of the index
	type Index = u32;
	//type of the call
	type Call = Call;
	//type of the event
	type Event = Event;
	//type of the signature
	type Signature = Signature; 

	fn new(number: Self::Number, parent_hash: Self::Hash, state_root: Self::Hash, extrinsics_root: Self::Hash, digest: Self::Digest) -> Self {
		BlockHeader {
			parent: parent_hash,
			timestamp: number,
			nonce: 0,
			contentRoot: extrinsics_root,
			extraContent: [0; 32],
			difficulty: state_root
		}
	}                     
}

impl BlockHeader {
	//create a new block header function
	pub fn new(parent: H256, timestamp: u128, nonce: u32, contentRoot: H256, extraContent: [u8; 32], difficulty: H256) -> Self {
		BlockHeader {
			parent,
			timestamp,
			nonce,
			contentRoot,
			extraContent,
			difficulty
		}
	}
}

//Hashing function for the block header using Blake3
impl Hash for BlockHeader {
	fn hash(&self) -> H256 {
		let mut hasher = Blake3::Hasher::new();
		hasher.update(&self.parent);
		hasher.update(&self.timestamp.to_le_bytes());
		hasher.update(&self.nonce.to_le_bytes());
		hasher.update(&self.contentRoot);
		hasher.update(&self.extraContent);
		hasher.update(&self.difficulty);
		let hash = hasher.finalize();
		H256::from_slice(&hash.as_bytes())
	}
}

//create test to see if the hashing function works
#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_hash() {
		let parent = H256::from_low_u64_be(1);
		let timestamp = 1;
		let nonce = 1;
		let contentRoot = H256::from_low_u64_be(1);
		let extraContent = [0; 32];
		let difficulty = H256::from_low_u64_be(1);
		let header = BlockHeader::new(parent, timestamp, nonce, contentRoot, extraContent, difficulty);
		let hash = header.hash();
		assert_eq!(hash, H256::from_low_u64_be(1));
	}

	//create test to see if the new function works
	#[test]
	fn test_new() {
		let parent = H256::from_low_u64_be(1);
		let timestamp = 1;
		let nonce = 1;
		let contentRoot = H256::from_low_u64_be(1);
		let extraContent = [0; 32];
		let difficulty = H256::from_low_u64_be(1);
		let header = BlockHeader::new(parent, timestamp, nonce, contentRoot, extraContent, difficulty);
		assert_eq!(header.parent, H256::from_low_u64_be(1));
		assert_eq!(header.timestamp, 1);
		assert_eq!(header.nonce, 1);
		assert_eq!(header.contentRoot, H256::from_low_u64_be(1));
		assert_eq!(header.extraContent, [0; 32]);
		assert_eq!(header.difficulty, H256::from_low_u64_be(1));
	}

}

