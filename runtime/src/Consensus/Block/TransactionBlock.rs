use sp_core::H256;
use super::Block;
//import extrinsics
use sp_runtime::traits::Extrinsic;

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