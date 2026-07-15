use argonaut_runtime::{AccountId, WASM_BINARY};
use sc_service::ChainType;
use sp_core::{sr25519, Pair, Public};

pub type ChainSpec = sc_chain_spec::GenericChainSpec;

// ── B-16: Prism consensus params live in the chain spec, not hardcoded in
// node/src/service.rs. This is deliberately narrow: only the OPERATIONAL
// tuning knobs (voter-chain count, starting difficulty, target block
// interval, stage-1 confirmation depth) are chain-spec-driven. B-11's
// `retarget_enabled = false` stays hardcoded in service.rs -- that's a
// consensus-SAFETY choice (per-node difficulty retargeting can permanently
// split the chain, see `Params::retarget_enabled`'s doc comment), not an
// operational one, and shouldn't be casually reconfigurable via a JSON
// property. ──────────────────────────────────────────────────────────────

/// JSON property keys read by [`prism_params_from_properties`] / written by
/// [`with_prism_properties`]. Kept as named consts so the two stay in sync.
mod prism_property_keys {
    pub const M_VOTER_CHAINS: &str = "prismMVoterChains";
    /// `difficulty = U256::MAX >> shift`. Replaces the old two-step
    /// `Params::testnet()`'s implicit `>>20` followed by service.rs's
    /// additional `>>4` (B-11) with one configurable value; the default
    /// (24 = 20+4) reproduces exactly what those two steps used to compute.
    pub const DIFFICULTY_SHIFT: &str = "prismDifficultyShift";
    pub const TARGET_INTERVAL_MS: &str = "prismTargetIntervalMs";
    /// Stage-1's k-deep confirmation threshold
    /// (`argonaut_prism_ledger::K_DEEP_CONFIRMATION`'s runtime value).
    pub const K_DEEP_CONFIRMATION: &str = "prismKDeepConfirmation";
}

/// Devnet defaults -- byte-for-byte what this codebase hardcoded before B-16
/// (`Params::testnet()`'s `m=100`/`target_interval_ms=1000`, `service.rs`'s
/// old `difficulty >> 4` dev-calibration shift on top of `testnet()`'s own
/// `>>20`, and `argonaut_prism_ledger::K_DEEP_CONFIRMATION = 6`). Changing
/// these changes the devnet's mining/confirmation behavior for every chain
/// spec built with [`prism_properties`] -- not a decision to make lightly;
/// see B-16's entry in `docs/IMPLEMENTATION_AB_FIXPLAN.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrismChainSpecParams {
    pub m_voter_chains: u16,
    pub difficulty_shift: u32,
    pub target_interval_ms: u64,
    pub k_deep_confirmation: u64,
}

impl Default for PrismChainSpecParams {
    fn default() -> Self {
        // Derive from the existing sources of truth rather than re-hardcoding
        // matching literals here (that would just be a second, silently
        // driftable copy of the same numbers): `m_voter_chains`/
        // `target_interval_ms` from `Params::testnet()`, `k_deep_confirmation`
        // from `argonaut_prism_ledger::K_DEEP_CONFIRMATION`. Only
        // `difficulty_shift` has no prior single source of truth (it replaces
        // the old two-step `testnet()`'s `>>20` + service.rs's `>>4`), so it's
        // the sum of those two as a plain literal.
        let testnet = argonaut_primitives_prism::Params::testnet();
        PrismChainSpecParams {
            m_voter_chains: testnet.m_voter_chains,
            difficulty_shift: 24, // == 20 (old Params::testnet() shift) + 4 (old B-11 service.rs shift)
            target_interval_ms: testnet.target_interval_ms,
            k_deep_confirmation: argonaut_prism_ledger::K_DEEP_CONFIRMATION,
        }
    }
}

/// Build the `properties` JSON object for [`PrismChainSpecParams::default`],
/// to pass to `.with_properties(...)` in the `ChainSpec::builder(...)` chain.
pub fn prism_properties() -> sc_chain_spec::Properties {
    use prism_property_keys::*;
    let params = PrismChainSpecParams::default();
    let mut properties = sc_chain_spec::Properties::new();
    properties.insert(M_VOTER_CHAINS.into(), (params.m_voter_chains as u64).into());
    properties.insert(DIFFICULTY_SHIFT.into(), (params.difficulty_shift as u64).into());
    properties.insert(TARGET_INTERVAL_MS.into(), params.target_interval_ms.into());
    properties.insert(K_DEEP_CONFIRMATION.into(), params.k_deep_confirmation.into());
    properties
}

/// Read Prism params back out of a chain spec's `properties`. Missing or
/// malformed individual fields fall back to [`PrismChainSpecParams::default`]'s
/// corresponding value (e.g. for a hand-edited/imported raw chain-spec JSON
/// file that predates B-16) -- never hard-errors, since these are operational
/// tuning knobs, not something a malformed value should crash node startup
/// over.
pub fn prism_params_from_properties(properties: &sc_chain_spec::Properties) -> PrismChainSpecParams {
    use prism_property_keys::*;
    let defaults = PrismChainSpecParams::default();
    PrismChainSpecParams {
        m_voter_chains: properties
            .get(M_VOTER_CHAINS)
            .and_then(|v| v.as_u64())
            .and_then(|v| u16::try_from(v).ok())
            .unwrap_or(defaults.m_voter_chains),
        difficulty_shift: properties
            .get(DIFFICULTY_SHIFT)
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(defaults.difficulty_shift),
        target_interval_ms: properties
            .get(TARGET_INTERVAL_MS)
            .and_then(|v| v.as_u64())
            .unwrap_or(defaults.target_interval_ms),
        k_deep_confirmation: properties
            .get(K_DEEP_CONFIRMATION)
            .and_then(|v| v.as_u64())
            .unwrap_or(defaults.k_deep_confirmation),
    }
}

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
    .with_properties(prism_properties())
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
    .with_properties(prism_properties())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// B-16: writing the default params into `properties` and reading them
    /// back must reproduce the exact same struct -- the whole point of this
    /// refactor is that `service.rs` sees byte-identical values to what was
    /// hardcoded before, when a chain spec doesn't override anything.
    #[test]
    fn properties_round_trip_the_defaults() {
        let written = PrismChainSpecParams::default();
        let properties = prism_properties();
        let read_back = prism_params_from_properties(&properties);
        assert_eq!(written, read_back);
    }

    /// A chain spec built before B-16 (or any hand-written one missing these
    /// keys) must fall back to the defaults, not panic or silently zero
    /// things out.
    #[test]
    fn missing_properties_fall_back_to_defaults() {
        let empty = sc_chain_spec::Properties::new();
        assert_eq!(prism_params_from_properties(&empty), PrismChainSpecParams::default());
    }

    /// A malformed value (wrong JSON type) for one key falls back to just
    /// that key's default, not a hard error -- these are operational tuning
    /// knobs, not something a typo should crash node startup over.
    #[test]
    fn malformed_single_field_falls_back_only_for_that_field() {
        let mut properties = prism_properties();
        properties.insert(
            prism_property_keys::M_VOTER_CHAINS.into(),
            serde_json::Value::String("not a number".into()),
        );
        let read_back = prism_params_from_properties(&properties);
        let defaults = PrismChainSpecParams::default();
        assert_eq!(read_back.m_voter_chains, defaults.m_voter_chains, "malformed field falls back");
        assert_eq!(read_back.difficulty_shift, defaults.difficulty_shift, "other fields unaffected");
        assert_eq!(read_back.target_interval_ms, defaults.target_interval_ms);
        assert_eq!(read_back.k_deep_confirmation, defaults.k_deep_confirmation);
    }

    /// An explicit override actually takes effect (not just "defaults always
    /// win") -- the entire point of moving this to the chain spec.
    #[test]
    fn explicit_override_is_honored() {
        let mut properties = prism_properties();
        properties.insert(prism_property_keys::M_VOTER_CHAINS.into(), 4u64.into());
        let read_back = prism_params_from_properties(&properties);
        assert_eq!(read_back.m_voter_chains, 4);
        // Untouched fields still come from the written defaults.
        assert_eq!(read_back.target_interval_ms, PrismChainSpecParams::default().target_interval_ms);
    }
}
