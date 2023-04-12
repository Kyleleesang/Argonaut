use sp_core::H256;
use sp_runtime::traits::{Extrinsic, Block, Hash};


#[derive(Serialize, Deserialize, Clone, Copy, Hash, Default)]
pub struct Content{
	//a vector of extrinsics
	pub extrinsics: Vec<Extrinsic>,
}
//impl content
impl Content{
	//create a new content function
	pub fn new(extrinsics: Vec<Extrinsic>) -> Self {
		Content {
			extrinsics
		}
	}
}

impl Block for Content {
	//type of the hash
	type Hash = H256;
	//type of the extrinsic
	type Extrinsic = Extrinsic;
	type Header = Header;
	//create a function to return the extrinsics
	fn extrinsics(&self) -> &[Self::Extrinsic] {
		&self.extrinsics
	}
}

impl PayloadSize for Content {
	fn payload_size(&self) -> usize {
		//iterate through the list of extrinsics and add up all of their sizes into a usize
		let mut size = 0;
		for extrinsic in self.extrinsics {
			size += extrinsic.size();
		}
	}
}


impl Hashable for Content{
	fn hash(&self) -> H256{
//create a new merkle tree from the Extrinsics vector
		let mut tree = MerkleTree::new(self.extrinsics);
		let root = tree.root();
		root
	}
}