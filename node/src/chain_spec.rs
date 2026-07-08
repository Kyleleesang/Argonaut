use argonaut_runtime::{AccountId, WASM_BINARY};
use sc_service::ChainType;
use sp_core::{sr25519, Pair, Public};

pub type ChainSpec = sc_chain_spec::GenericChainSpec;

pub fn get_from_seed<TPublic: Public>(seed: &str) -> <TPublic::Pair as Pair>::Public {
    TPublic::Pair::from_string(&format!("//{}", seed), None)
        .expect("static values are valid; qed")
        .public()
}

/// Derives a dev account's `AccountId` via `sp_core`'s own
/// `AccountId32: From<sr25519::Public>` conversion -- deliberately NOT
/// through `ArgonautSigner`/`ArgonautSignature::Sr25519`, which requires the
/// `dev-signatures` Cargo feature. Chain-spec account derivation doesn't
/// need that feature at all; routing it through `ArgonautSigner` anyway was
/// what forced every node build (including the production wasm blob) to
/// activate `dev-signatures`, since `substrate-wasm-builder` mirrors
/// whatever features are active on `argonaut-runtime`'s own native build
/// into the separate wasm build it spawns -- found live, 2026-07-07 (fix
/// plan A-1: the production-wasm-rejects-sr25519 claim had never actually
/// been verified). Produces byte-identical `AccountId`s to the old
/// `ArgonautSigner::Sr25519(pk).into_account()` path, since that path just
/// called this same `sp_core` conversion under the hood.
pub fn get_account_id_from_seed<TPublic: Public>(seed: &str) -> AccountId
where
    AccountId: From<<TPublic::Pair as Pair>::Public>,
{
    AccountId::from(get_from_seed::<TPublic>(seed))
}

pub fn development_config() -> Result<ChainSpec, String> {
    Ok(ChainSpec::builder(
        WASM_BINARY.ok_or_else(|| "Development wasm not available".to_string())?,
        Default::default(),
    )
    .with_name("Argonaut Development")
    .with_id("dev")
    .with_chain_type(ChainType::Development)
    .with_genesis_config_patch(testnet_genesis(
        get_account_id_from_seed::<sr25519::Public>("Alice"),
        vec![
            get_account_id_from_seed::<sr25519::Public>("Alice"),
            get_account_id_from_seed::<sr25519::Public>("Bob"),
            get_account_id_from_seed::<sr25519::Public>("Alice//stash"),
            get_account_id_from_seed::<sr25519::Public>("Bob//stash"),
        ],
    ))
    .build())
}

pub fn local_testnet_config() -> Result<ChainSpec, String> {
    Ok(ChainSpec::builder(
        WASM_BINARY.ok_or_else(|| "Local testnet wasm not available".to_string())?,
        Default::default(),
    )
    .with_name("Argonaut Local Testnet")
    .with_id("local_testnet")
    .with_chain_type(ChainType::Local)
    .with_genesis_config_patch(testnet_genesis(
        get_account_id_from_seed::<sr25519::Public>("Alice"),
        vec![
            get_account_id_from_seed::<sr25519::Public>("Alice"),
            get_account_id_from_seed::<sr25519::Public>("Bob"),
            get_account_id_from_seed::<sr25519::Public>("Charlie"),
            get_account_id_from_seed::<sr25519::Public>("Dave"),
            get_account_id_from_seed::<sr25519::Public>("Eve"),
            get_account_id_from_seed::<sr25519::Public>("Ferdie"),
            get_account_id_from_seed::<sr25519::Public>("Alice//stash"),
            get_account_id_from_seed::<sr25519::Public>("Bob//stash"),
            get_account_id_from_seed::<sr25519::Public>("Charlie//stash"),
            get_account_id_from_seed::<sr25519::Public>("Dave//stash"),
            get_account_id_from_seed::<sr25519::Public>("Eve//stash"),
            get_account_id_from_seed::<sr25519::Public>("Ferdie//stash"),
        ],
    ))
    .build())
}

// No Aura/GRANDPA authority set to seed: Prism (client-side consensus, see
// architecture guide §3) determines block content, and the B3 bridge
// (node/src/bridge.rs) imports every Substrate block already finalized --
// there's no separate finality gadget to configure.
fn testnet_genesis(root_key: AccountId, endowed_accounts: Vec<AccountId>) -> serde_json::Value {
    serde_json::json!({
        "balances": {
            "balances": endowed_accounts.iter()
                .cloned()
                .map(|k| (k, 1u64 << 60))
                .collect::<Vec<_>>()
        },
        "sudo": { "key": Some(root_key) }
    })
}
