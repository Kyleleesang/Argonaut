use sp_core::H256;
use sp_runtime::{
    generic::{BlockId, Block, Header},
    traits::{NumberFor, Extrinsic},
};

use raptorq::Encoder;

#[derive(Default, Clone, PartialEq, Eq)]
pub struct ProposerBlock{
    pub inner: Block,
    pub Content: Content
}

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
	//create function to return the transaction references and proposer references
	pub fn getTransactionReferences(&self) -> Vec<H256>{
		self.transactionReferences
	}
	pub fn getProposerReferences(&self) -> Vec<H256>{
		self.proposerReferences
	}
}




impl Block for ProposerBlock {
    type Hash = Block::Hash;
    type Header = Header;
    type Extrinsic = Extrinsic;

    fn header(&self) -> &Self::Header {
        &self.inner.header()
    }

    fn extrinsics(&self) -> &[Self::Extrinsic] {
        &self.inner.extrinsics
    }

    fn decode_extrinsic(bytes: &[u8]) -> Result<Self::Extrinsic, codec::Error> {
        Extrinsic::decode(bytes)
    }

    fn decode_header(bytes: &[u8]) -> Result<Self::Header, codec::Error> {
        Header::decode(bytes)
    }

    fn decode_block(bytes: &[u8]) -> Result<Self, codec::Error> {
        let inner = Block::decode(bytes)?;
        let extrinsics: Vec<Extrinsic> = inner.extrinsics().iter().cloned().map(|x| x.decode()).collect::<Result<Vec<_>, _>>()?;
        Ok(ProposerBlock {
            inner,
            extrinsics,
            proposer_chain_head,
            proposer_chain_difficulty,
			proposer_chain_nonce,
            proposer_chain_extra_data,
            encoded_data,
        })
    }

    fn encode_sealed(header: &Self::Header, body: &[Self::Extrinsic]) -> Vec<u8> {
        let mut encoded_data: Vec<u8> = Vec::new();
        let mut encoder = Encoder::new(&mut encoded_data).unwrap();
        encoder.encode_all(body).unwrap();

        let mut encoded_block = Block::encode_sealed(header, &encoded_data);
        encoded_block.extend_from_slice(&self.proposer_chain_head);
        encoded_block.extend_from_slice(&self.proposer_chain_difficulty.to_le_bytes());
        encoded_block.extend_from_slice(&self.proposer_chain_nonce.to_le_bytes());
        encoded_block.extend(self.proposer_chain_extra_data.clone());
        encoded_block
    }


 fn Genesis() -> ProposerBlock{
	//create a new content with the transaction and voter vectors being empty
	let content = Content::new(Vec::new(), Vec::new());
	//hash the content using Blake3 hasher
	let mut hasher = Blake3::Hasher::new();
	hasher.update(&content.hash());
	let hash = hasher.finalize();
	//create a new block with the hash of the content, the timestamp being the current, the nonce being 0, the extra content being 0, 
	//and the difficulty being 0 and return the Block
	let block = Block::new(H256::from_slice(&hash.as_bytes()), Timestamp::now(), 
							0, content.hash(), vec![], content, H256::from_low_u64_be(0), H256::from_low_u64_be(0));
	block
}

//create test for the genesis block to see if it returns a block
#[test]
fn testGenesis(){
	let block = Genesis();
	assert_eq!(block.header.parent, H256::from_low_u64_be(0));
	assert_eq!(block.header.timestamp, Timestamp::now());
	assert_eq!(block.header.nonce, 0);
	assert_eq!(block.header.content_merkleroot, block.content.hash());
	assert_eq!(block.header.extraContent, Vec::new());
	assert_eq!(block.header.difficulty, H256::from_low_u64_be(0));
}

}
