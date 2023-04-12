use sp_runtime::traits::{Block as BlockT, Header as HeaderT, DigestItem as Digest};
use sp_core::H256;
pub mod Header;
pub mod TransactionBlock;
pub mod VoterBlock;



#[derive(Hash, Copy, Serialize, Deserialize)]
pub struct Block{
	//The Block's header
	pub header: Header::BlockHeader,
	pub content: Content,
	//sortition proof needed in addition to the content merkle root in the block header needed to verify the block
	//was mined on a set of candidates 
	pub sortitionProof: Vec<H256>,
	pub coinbase: AccountId
}
//create a new stuct called proposerBlock that inherits from BlockT
impl BlockT for Block {
	//type of the header
	type Header = Header::BlockHeader;
	//type of the hash
	type Hash = H256;
	//type of the hash
	type Hashing = Blake3;
	//type of the digest
	type Digest = Digest;
	//type of the extrinsic
	type Extrinsic = Extrinsic;
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

	fn Extrinsics() -> &[Self::Extrinsic] {
		//match the context with the block type
		match self {
			//if the block is a proposer block
			Block::ProposerBlock(block) => {
				//return the extrinsics
				block.content.getTransactionReferences() + block.content.getVoterReferences();
			}
			//if the block is a voter block
			Block::VoterBlock(block) => {
				//return the extrinsics
				block.content.extrinsics
			}

			Block::TransactionBlock(block) => {
				//return the extrinsics
				block.content.extrinsics
			}
		}
	}
	
	fn new(Parent: H256, Timestamp: u128, nonce: u32, content_merkleroot: H256, 
			sortitionProof: Vec<H256>, content: Content, ExtraContent: Vec<Content>, 
			difficulty: H256) -> Self{
		let Header = Header::new(Parent, Timestamp, nonce, content_merkleroot, ExtraContent, difficulty);
		Block{header: Header, content: content, SortitionProof}
	}
	//deconstruct method to split the block into a header and its extrinsics
	fn deconstruct(self) -> (Self::Header, Vec<Self::Extrinsic>) {
		//match the context with the block type
		match self {
			//if the block is a proposer block
			Block::ProposerBlock(block) => {
				//return the header and extrinsics
				(block.header, block.content.getTransactionReferences() + block.content.getVoterReferences())
			}
			//if the block is a voter block
			Block::VoterBlock(block) => {
				//return the header and extrinsics
				(block.header, block.content.extrinsics)
			}

			Block::TransactionBlock(block) => {
				//return the header and extrinsics
				(block.header, block.content.extrinsics)
			}
		}
	}
	//Create an encoded block from the given header and extrinsics without requiring the creation of an instance
	fn encode_from(header: &Self::Header, extrinsics: &[Self::Extrinsic]) -> Vec<u8> {
		//match the context with the block type
		match self {
			//if the block is a proposer block
			Block::ProposerBlock(block) => {
				//return the header and extrinsics
				(block.header, block.content.getTransactionReferences() + block.content.getVoterReferences())
			}
			//if the block is a voter block
			Block::VoterBlock(block) => {
				//return the header and extrinsics
				(block.header, block.content.extrinsics)
			}

			Block::TransactionBlock(block) => {
				//return the header and extrinsics
				(block.header, block.content.extrinsics)
			}
		}
	}

	fn header(&self) -> &Self::Header {
		&self.header
	}
}



//potentially change extra content to a Vec[u8] instead later
impl Block{
	fn new(Parent: H256, Timestamp: u128, nonce: u32, content_merkleroot: H256,
		   sortitionProof: SortitionProof, content: Content, ExtraContent: Vec<Content>, 
		   difficulty: H256) -> Self{
		let Header = Header::new(Parent, Timestamp, nonce, content_merkleroot, ExtraContent, difficulty);
		Block{
			header: Header,
			content: content,
			SortitionProof
			//
		}
	}
	pub fn fromHeader(header: Header, SortitionProof: sortitionProof, content: Content) -> Self{
		Block{
			header: header,
			content: content,
			SortitionProof
		}
	}

	//implement hashable for Block
	impl Hashable for Block{
		fn hash(&self) -> H256{
			let mut hasher = Blake3::Hasher::new();
			hasher.update(&self.header);
			hasher.update(&self.content);
			hasher.update(&self.sortitionProof);
			let hash = hasher.finalize();
			H256::from_slice(&hash.as_bytes())
		}
	}

	//implement digest item for block
	impl DigestItem for Block{
		fn digest(&self) -> Digest{
			Digest::from(self.hash())
		}
	}

	//implement payloadSize for block
	impl PayloadSize for Block{
		fn payloadSize(&self) -> usize{
			self.header.size() + self.content.size() + self.sortitionProof.len() + size()
		}
	}
}


pub enum Content{
	TransactionBlock(TransactionBlock::Content),
	ProposerBlock(ProposerBlock::Content),
	VoterBlock(VoterBlock::Content)
}
