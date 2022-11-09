

pub struct Content{
	//list of transaction blocks referred by this proposer block
	pub transactionReferences: Vec<H256>,
	//list of proposer blocks referred by this proposer block
	pub proposerReferences: Vec<H256>,
}

impl Content{
	pub fn new(transactionReferences: Vec<H256>, proposerReferences: Vec<H256>) -> Self{
		Content{
			transactionReferences,
			proposerReferences,
		}
	}
}

impl PayloadSize for Content{
	fn payloadSize(&self) -> usize{
		//iterate through the list of transaction references and add up all of their sizes into a usize
		let mut size = 0;
		//take the size of an H256 and multiply it by the number of transaction references and proposer references
		size += size(<H256>) * (self.transactionReferences.len() + self.proposerReferences.len());
	}
}

impl Hashable for Content{
	fn hash(&self) -> H256{
//create a new merkle tree from the transactionReferences 
		let mut tree = MerkleTree::new(self.transactionReferences);
		let TxRoot = tree.root();
//create a merkle tree from the proposerReferences
		let mut tree = MerkleTree::new(self.proposerReferences);
		let ProposerRoot = tree.root();
		//make a 64 byte array
		let mut hash = [0; 64];
		//copy the TxRoot into the first 32 bytes of the hash
		hash[0..32].copy_from_slice(&TxRoot);
		//copy the ProposerRoot into the last 32 bytes of the hash
		hash[32..64].copy_from_slice(&ProposerRoot);
		//take the Blake3 hash of the array and return it
		let mut hasher = Blake3::Hasher::new();
		hasher.update(&hash);
		let hash = hasher.finalize();
		let returnHash = H256::from_slice(&hash.as_bytes());
		returnHash
	}
}

pub fn Genesis() -> Block{
	//create a new content with the transaction and voter vectors being empty
	let content = Content::new(Vec::new(), Vec::new());
	//hash the content using Blake3 hasher
	let mut hasher = Blake3::Hasher::new();
	hasher.update(&content.hash());
	let hash = hasher.finalize();
	//create a new block with the hash of the content, the timestamp being the current, the nonce being 0, the extra content being 0, 
	//and the difficulty being 0 and return the Block
	let block = Block::new(H256::from_slice(&hash.as_bytes()), Timestamp::now(), 0, content, [0u8; 32], H256::from_low_u64_be(0));
	block
}