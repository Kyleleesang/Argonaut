use super::Block;
use super::Content as BlockContent;
//use sp_core::H256;
//import Blake3 from the Blake3 crate
use blake3::*;
//import sp timestamp
use sp_timestamp::Timestamp;



#[derive(Serialize, Deserialize, Clone, Copy, Hash, Default)]
pub struct Content{
	//ID of the voter chain
	pub chainNumber: u16,
	//Hash of the parent voter block
	pub parent: H256,
	//list of votes on the proposer blocks
	pub votes: Vec<H256>,
}

//implement hashable for Content

impl Hashable for Content{
	fn hash(&self) -> H256{
		//create a merkle tree from the votes
		let mut tree = MerkleTree::new(self.votes);
		let root = tree.root();
		//declare a byte array with the first 2 bytes to be the chain number, the next 32 to be from the parent, and the last 32 to be from the root
		let mut bytes = [0u8; 66];
		bytes[0] = (self.chainNumber >> 8) as u8;
		bytes[1] = self.chainNumber as u8;
		bytes[2..34].copy_from_slice(&self.parent[..]);
		bytes[34..66].copy_from_slice(&root[..]);
		//hash the byte array
		let hash = Blake3::hash(&bytes);
		H256::from_slice(&hash.as_bytes())
	}
}

impl Content {
	//create a new content function
	pub fn new(chainNumber: u16, parent: H256, votes: Vec<H256>) -> Self {
		Content {
			chainNumber,
			parent,
			votes
		}
	}
}
//TODO later: get the genesis working with input content, check to see if the contentMerkleRoot should be from hash or merkle root of all votes
pub fn Genesis(chainNumber: u16) -> Block {
	//create a new content with the chain number 0, the parent being the hash of 0, and the votes being an empty vector
	let content = Content::new(chainNumber, H256::from_low_u64_be(0), Vec::new());
	//hash the content using Blake3 hasher
	let mut hasher = Blake3::Hasher::new();
	hasher.update(&content.hash());
	let contentMerkleRoot = hasher.finalize();
	//create a new block with the hash of the content, the timestamp being the current, the nonce being 0, the extra content being 0, 
	//and the difficulty being 0 and return the Block
	let block = Block::new(Content::Parent, Timestamp::now(), 0, content.hash(), vec![], content, H256::from_low_u64_be(0), 0);
	return block;
}