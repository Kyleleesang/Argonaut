
use frame_support::storage::*;
use Blake3::*;
use sp_blockchain::{HeaderBackend, Backend};


// Column family names for node/chain metadata
const PROPOSER_NODE_LEVEL_CF: &str = "PROPOSER_NODE_LEVEL"; // hash to node level (u64)
const VOTER_NODE_LEVEL_CF: &str = "VOTER_NODE_LEVEL"; // hash to node level (u64)
const VOTER_NODE_CHAIN_CF: &str = "VOTER_NODE_CHAIN"; // hash to chain number (u16)
const VOTER_TREE_LEVEL_COUNT_CF: &str = "VOTER_TREE_LEVEL_COUNT_CF"; // chain number and level (u16, u64) to number of blocks (u64)
const PROPOSER_TREE_LEVEL_CF: &str = "PROPOSER_TREE_LEVEL"; // level (u64) to hashes of blocks (Vec<hash>)
const VOTER_NODE_VOTED_LEVEL_CF: &str = "VOTER_NODE_VOTED_LEVEL"; // hash to max. voted level (u64)
const PROPOSER_NODE_VOTE_CF: &str = "PROPOSER_NODE_VOTE"; // hash to level and chain number of main chain votes (Vec<u16, u64>)
const PROPOSER_LEADER_SEQUENCE_CF: &str = "PROPOSER_LEADER_SEQUENCE"; // level (u64) to hash of leader block.
const PROPOSER_LEDGER_ORDER_CF: &str = "PROPOSER_LEDGER_ORDER"; // level (u64) to the list of proposer blocks confirmed
// by this level, including the leader itself. The list
// is in the order that those blocks should live in the ledger.
const PROPOSER_VOTE_COUNT_CF: &str = "PROPOSER_VOTE_COUNT"; // number of all votes on a block

// Column family names for graph neighbors
const PARENT_NEIGHBOR_CF: &str = "GRAPH_PARENT_NEIGHBOR"; // the proposer parent of a block
const VOTE_NEIGHBOR_CF: &str = "GRAPH_VOTE_NEIGHBOR"; // neighbors associated by a vote
const VOTER_PARENT_NEIGHBOR_CF: &str = "GRAPH_VOTER_PARENT_NEIGHBOR"; // the voter parent of a block
const TRANSACTION_REF_NEIGHBOR_CF: &str = "GRAPH_TRANSACTION_REF_NEIGHBOR";
const PROPOSER_REF_NEIGHBOR_CF: &str = "GRAPH_PROPOSER_REF_NEIGHBOR";

pub type Result<T> = std::result::Result<T, rocksdb::Error>;



pub struct BlockTree{
	pub Blocks: StorageMap<Blake3, H256, Block>,
	proposerBestLevel: Mutex<u64>,
	pub voterBest: Vec<Mutex<(H256, u128)>>,
	pub unreferredTransactions: Mutex<StorageMap<Blake3, H256, u128>>,
	pub unreferredProposers: Mutex<StorageMap<Blake3, H256, u128>>,
	//create voter ledger tips a mutex vector of hashes
	pub voterLedgerTips: Mutex<Vec<H256>>,
	//proposer ledger tip is a mutex u64
	pub proposerLedgerTip: Mutex<u64>,
	config: BlockchainConfig,
}

impl Backend for BlockTree{

	 fn body(&self, id: &BlockId<Block>) -> sp_blockchain::Result<Option<Vec<<Block as BlockT>::Extrinsic>>> {
		Ok(self.Blocks.get(id).map(|b| b.extrinsics().to_vec()))
	}

	fn justification(&self, id: &BlockId<Block>) -> sp_blockchain::Result<Option<Justification>> {
		Ok(self.Blocks.get(id).map(|b| b.justification().to_vec()))
	}

	fn finalize_block(&self, id: &BlockId<Block>, justification: Option<Justification>, _body: Option<Vec<<Block as BlockT>::Extrinsic>>) -> sp_blockchain::Result<()> {
		let mut block = self.Blocks.get(id).ok_or(sp_blockchain::Error::UnknownBlock(format!("{:?}", id)))?;
		block.set_justification(justification);
		self.Blocks.insert(id, block);
		Ok(())
	}
	//dead function for now
	fn lastFinalized(&self) -> sp_blockchain::Result<BlockId<Block>> {
		Ok(BlockId::Hash(self.Blocks.get(&BlockId::Number(Zero::zero())).unwrap().hash()))
	}
	//make a function called leaves that returns a vector of hashes of all the blocks that are leaves in the tree. In other words, all the blocks that have no children
	fn leaves(&self) -> sp_blockchain::Result<Vec<BlockId<Block>>> {
		//create a vector of hashes
		let mut leaves = Vec::new();
		//iterate through the blocks in the tree
		for block in self.Blocks.iter() {
			//if the block has no children
			if self.Blocks.get(&BlockId::Hash(block.parent_hash())).is_none() {
				//add the block to the vector of hashes
				leaves.push(BlockId::Hash(block.1.hash()));
			}
		}
		//return the vector of hashes
		Ok(leaves)
	}

	//returns a vector of hashes of all the children of that block
	fn children(&self, id: &BlockId<Block>) -> sp_blockchain::Result<Vec<BlockId<Block>>> {
		//create a vector of hashes
		let mut children = Vec::new();
		//iterate through the blocks in the tree
		for block in self.Blocks.iter() {
			//if the block has the given hash as its parent
			if block.parent_hash() == self.Blocks.get(id).unwrap().hash() {
				//add the block to the vector of hashes
				children.push(BlockId::Hash(block.1.hash()));
			}
		}
		//return the vector of hashes
		Ok(children)
	}
	fn indexedTransaction(&self, id: &BlockId<Block>) -> sp_blockchain::Result<Option<Vec<<Block as BlockT>::Extrinsic>>> {
		Ok(self.Blocks.get(id).map(|b| b.extrinsics().to_vec()))
	}

}