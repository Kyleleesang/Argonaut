use sc_cli::RunCmd;

#[derive(Debug, clap::Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub subcommand: Option<Subcommand>,

    #[command(flatten)]
    pub run: RunCmd,

    /// Enable B5 pruning of old Prism DAG history (`client/prism-chain`'s
    /// `PrismChain::prune`): voter blocks beyond `--prism-k-prune`, and
    /// proposer/tx-block bodies for already-confirmed levels. Off by
    /// default per the pruning design's own operational note
    /// (`docs/IMPLEMENTATION_AB_FIXPLAN.md`, "Then: B5 (pruning)"): a
    /// pruning node can't serve full historical backfill (B-6) to a fresh
    /// peer, so every devnet should keep at least one non-pruning archive
    /// node running with this flag unset.
    #[arg(long)]
    pub prism_prune: bool,

    /// How many blocks behind a voter chain's own tip before that history
    /// becomes prunable (only read when `--prism-prune` is set). Default is
    /// a large (10x) multiple of `K_DEEP_CONFIRMATION`, matching the
    /// pruning design's guidance that this must stay well above stage-1
    /// confirmation's own reorg-depth assumption.
    #[arg(long, default_value_t = 60)]
    pub prism_k_prune: u64,

    /// Enable stage-2 confidence-interval fast confirmation
    /// (`client/prism-ledger`'s `confirmation::confirm_unique_leader`), which
    /// otherwise races stage-1's conservative k-deep rule ahead of it. **Off
    /// by default (B-12):** stage-2's live `delta_d` is a flagged placeholder
    /// (B-3), not the paper's derived operating point, so with this on, the
    /// bridge's irreversible finality rests on an unproven confidence margin.
    /// Exists mainly to measure/compare confirmed-level latency with stage-2
    /// on vs. off (B-4) -- see `docs/IMPLEMENTATION_AB_FIXPLAN.md`'s B-12
    /// entry before enabling this on anything but a throwaway devnet.
    #[arg(long)]
    pub prism_stage2: bool,
}

#[derive(Debug, clap::Subcommand)]
pub enum Subcommand {
    /// Key management CLI utilities.
    #[command(subcommand)]
    Key(sc_cli::KeySubcommand),

    /// Falcon-512 key management (generate/inspect keystore files). Stock `key` above
    /// only supports ed25519/sr25519/ecdsa; Falcon needs its own subcommand.
    #[command(subcommand)]
    FalconKey(crate::keygen::FalconKeySubcommand),

    /// Sign a payload offline with a Falcon-512 keystore (bootstrap-mode only).
    FalconSign(crate::falcon_sign::FalconSignCmd),

    /// Build a chain specification.
    BuildSpec(sc_cli::BuildSpecCmd),

    /// Validate blocks.
    CheckBlock(sc_cli::CheckBlockCmd),

    /// Export blocks.
    ExportBlocks(sc_cli::ExportBlocksCmd),

    /// Export the state of a given block into a chain spec.
    ExportState(sc_cli::ExportStateCmd),

    /// Import blocks.
    ImportBlocks(sc_cli::ImportBlocksCmd),

    /// Remove the whole chain.
    PurgeChain(sc_cli::PurgeChainCmd),

    /// Revert the chain to a previous state.
    Revert(sc_cli::RevertCmd),

    /// Sub-commands concerned with benchmarking.
    #[command(subcommand)]
    Benchmark(frame_benchmarking_cli::BenchmarkCmd),

    /// Try some command against runtime state. Requires `--features try-runtime`.
    #[cfg(feature = "try-runtime")]
    TryRuntime(try_runtime_cli::TryRuntimeCmd),

    /// Db meta columns information.
    ChainInfo(sc_cli::ChainInfoCmd),
}
