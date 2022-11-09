use sp_runtime::traits::{Block as BlockT, Header as HeaderT, DigestItem as Digest};
use sp_core::H256;
pub mod Header;
pub mod TransactionBlock;
pub mod VoterBlock;



#[derive(Hash, Copy, Serialize, Deserialize)]
pub struct Block{
	//The Block's header
	pub header: HeaderT,
	pub content: Content,
	//sortition proof needed in addition to the content merkle root in the block header needed to verify the block
	//was mined on a set of candidates 
	pub sortitionProof: Vec<H256>,
	pub coinbase: AccountId
}
//create a new stuct called proposerBlock that inherits from BlockT
impl BlockT for Block {
	//type of the header
	type Header = HeaderT;
	//type of the hash
	type Hash = H256;
	//type of the hash
	type Hashing = Blake3;
	//type of the digest
	type Digest = Digest;
	//type of the extrinsic
	type Extrinsic = BlockT::Extrinsic;
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
	//type of the account id
	type AccountId = AccountId;

	fn Extrinsics() -> &[Self::Extrinsic] {
		&[]
	}
	fn new(Parent: H256, Timestamp: u128, nonce: u32, content_merkleroot: H256, sortitionProof: SortitionProof, content: Content, ExtraContent: Vec<Content>, difficulty: H256) -> Self{
		let Header = Header::new(Parent, Timestamp, nonce, content_merkleroot, ExtraContent, difficulty);
		Block{
			header: Header,
			content: content,
			SortitionProof
		}
	}

}



//potentially change extra content to a Vec[u8] instead later
impl Block{
	fn new(Parent: H256, Timestamp: u128, nonce: u32, content_merkleroot: H256, sortitionProof: SortitionProof, content: Content, ExtraContent: Vec<Content>, difficulty: H256) -> Self{
		let Header = Header::new(Parent, Timestamp, nonce, content_merkleroot, ExtraContent, difficulty);
		Block{
			header: Header,
			content: content,
			SortitionProof
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
			self.header.size() + self.content.size() + self.sortitionProof.len() + size(<H256>)
		}
	}
}

pub enum Content{
	TransactionBlock(TransactionBlock::Content),
	ProposerBlock(ProposerBlock::Content),
	VoterBlock(VoterBlock::Content)
}
