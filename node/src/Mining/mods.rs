//import substrate transaction pool
use sp_transaction_pool::{InPoolTransaction, TransactionPool};
use sp_runtime::traits::{Block as BlockT, Header as HeaderT, NumberFor};
use sp_runtime::generic::BlockId;
use sp_runtime::transaction_validity::TransactionValidity;
use std::sync::Arc;
use sp_api::ProvideRuntimeApi;
use sp_blockchain::HeaderBackend;
use sp_consensus::{Error, BlockOrigin, SelectChain, Environment, Proposer};
use sc_consensus_pow::{Error as PowError, PowVerifier, PowAlgorithm, Seal, PoWBlockImport, Difficulty};




pub struct Blake3POW{
    pub client: Arc<Client>,
}

impl Blake3POW{
    pub fn new(client: Arc<Client>) -> Self{
        self{
            client,
        }
    }
}


impl<B: BlockT<Hash=H256>> PowAlgorithm<B> for Blake3POW{
    type Difficulty = U256;

    fn difficulty(&self, parent: B::Hash)-> Result<Self::Difficulty, Error<B>>{
        let parentID = BlockId::<B>::Hash(parent);
        self.client.runtime_api().difficulty(&parentID).map_err(|e| {sc_consensus_pow::Error::Environment(format!("{:?}", e)).into()});
    }




    fn verify(
        &self,
        _parent: &BlockId<B>,
        preHash: &H256,
        seal: &Seal,
        difficulty: Self::Difficulty,
    ) -> Result<bool, Error<B>>{
        let seal = match seal{
            Seal::Regular(seal) => seal,
            _ => return Ok(false),
        };

        if!hash_meets_difficulty(&seal.work, difficulty){
            return Ok(false);
        }
        let compute = Compute{
            difficulty,
            preHash: &preHash,
            nonce: &seal.nonce,
        };
        if compute.compute() != seal{
            return Ok(false);
        }
    }

    fn mine(
        &self,
        _parent: &BlockId<B>,
        preHash: &H256,
        difficulty: Self::Difficulty,
        _round: u32,
    ) -> Result<Option<Seal>, Error<B>>{
        let mut preHash = preHash.clone();
        let mut seal = Seal::default();
        let mut nonce = 0u64;
        loop{
            preHash.extend_from_slice(&seal);
            let hash = blake3::hash(&preHash);
            let hash = U256::from(hash.as_bytes());
            if hash < difficulty{
                break;
            }
            nonce += 1;
            seal = Seal::from(nonce.to_le_bytes());
        }
        Ok(Some(seal))
    }
}

impl sp_consensus::Environment<B> for Blake3POW{
    type Error = Error<B>;
    type Proposer = Blake3POWProposer<B>;
    type CreateProposer = Blake3POWProposerFactory<B>;

    fn init(&mut self, _parent_header: &B::Header) -> Result<(), Self::Error>{
        Ok(())
    }

    fn proposer(&self, _parent_header: &B::Header) -> Result<Self::CreateProposer, Self::Error>{
        Ok(Blake3POWProposerFactory{
            client: self.client.clone(),
        })
    }
}

pub struct Blake3POWProposerFactory<B: BlockT<Hash=H256>>{
    client: Arc<Client>,
}

impl<B: BlockT<Hash=H256>> sp_consensus::CreateProposer<B> for Blake3POWProposerFactory<B>{
    type Error = Error<B>;
    type Proposer = Blake3POWProposer<B>;

    fn init(&self, parent_header: &B::Header) -> Result<Self::Proposer, Self::Error>{
        let proposer = Blake3POWProposer{
            client: self.client.clone(),
            parent_header: parent_header.clone(),
        };
        Ok(proposer)
    }
}

