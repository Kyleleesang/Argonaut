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
use blake3::Hasher;




pub struct Blake3POW{
    pub client: Arc<Client>,
}

impl Blake3POW{
    pub fn new(client: Arc<Client>) -> Self{
        self{
            client,
        }
    }

    pub fn hash_meets_difficulty(hash: &H256, difficulty: U256) -> bool{
        let hash = U256::from(&hash[..]);
        let(_, overflowed) = hash.overflowing_mul(difficulty);
        !overflowed
    }

    #[derive(Encode, Decode, Clone, PartialEq, Eq, Debug)]
    pub struct Seal{
        pub difficulty: U256,
        pub work: H256,
        pub nonce: H256,
    }

    pub struct Compute<'a>{
        pub difficulty: U256,
        pub preHash: &'a H256,
        pub nonce: &'a H256,
    }

    impl Compute {
        pub fn compute(&self) -> Seal{
            let mut work = [0u8; 32];
            work.copy_from_slice(&blake3::hash(&[&self.preHash[..], &self.nonce[..]]).as_bytes()[..]);
            Seal{
                difficulty: self.difficulty,
                work: work.into(),
                nonce: self.nonce.clone(),
            }
        }
    }

impl <C> Clone for Blake3POW<C>{
    fn clone(&self) -> Self{
        Blake3POW{
            client: self.client.clone(),
        }
    }
}


impl<B: BlockT<Hash=H256>, C> PowAlgorithm<B> for Blake3POW where C: ProvideRuntimeApi<B>, C::Api: DifficultyApi<B>,{
    type Difficulty = U256;

    fn difficulty(&self, parent: B::Hash)-> Result<Self::Difficulty, Error<B>>{
        let parentID = BlockId::<B>::Hash(parent);
        self.client.runtime_api().difficulty(&parentID).map_err(|e| {sc_consensus_pow::Error::Environment(format!("{:?}", e)).into()});
    }

    fn verify(
        &self,
        _parent: &BlockId<B>,
        preHash: &H256,
        seal: &RawSeal,
        difficulty: Self::Difficulty,
    ) -> Result<bool, Error<B>>{
        let seal = match Seal::decode(&mut &seal[..]){
            ok(seal) => seal,
            Err(_) => return Ok(false),
        };
        //Check if the hash meets the difficulty if not then abort
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
}

