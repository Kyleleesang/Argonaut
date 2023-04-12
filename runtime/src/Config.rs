use frame_system::{pallet::Config, dispatch::Parameter};
use sp_runtime::AccountId32;
//TODO

struct ArgonautConfig{

}
#[derive(Encode, Decode, PartialEq, Eq, MaybeSerializeDeserialize, Parameter, Debug, MaybeDisplay, Parameter, Member, Ord)]
struct AccountID{

}

impl Config for ArgonautConfig{
    type AccountId = AccountId32; 

}