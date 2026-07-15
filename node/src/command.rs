#[cfg(feature = "dev-signatures")]
use crate::benchmarking::{inherent_benchmark_data, RemarkBuilder, TransferKeepAliveBuilder};
use crate::{
    chain_spec,
    cli::{Cli, Subcommand},
    service,
};
use frame_benchmarking_cli::{BenchmarkCmd, SUBSTRATE_REFERENCE_HARDWARE};
#[cfg(feature = "dev-signatures")]
use frame_benchmarking_cli::ExtrinsicFactory;
use argonaut_runtime::opaque::Block;
#[cfg(feature = "dev-signatures")]
use argonaut_runtime::EXISTENTIAL_DEPOSIT;
use clap::Parser;
use sc_cli::SubstrateCli;
use sc_service::PartialComponents;
#[cfg(feature = "dev-signatures")]
use sp_keyring::Sr25519Keyring;

impl SubstrateCli for Cli {
    fn impl_name() -> String { "Argonaut Node".into() }
    fn impl_version() -> String { env!("SUBSTRATE_CLI_IMPL_VERSION").into() }
    fn description() -> String { env!("CARGO_PKG_DESCRIPTION").into() }
    fn author() -> String { env!("CARGO_PKG_AUTHORS").into() }
    fn support_url() -> String { "https://github.com/Kyleleesang/Argonaut/issues".into() }
    fn copyright_start_year() -> i32 { 2024 }

    fn load_spec(&self, id: &str) -> Result<Box<dyn sc_service::ChainSpec>, String> {
        Ok(match id {
            "dev" => Box::new(chain_spec::development_config()?),
            "" | "local" => Box::new(chain_spec::local_testnet_config()?),
            path => Box::new(chain_spec::ChainSpec::from_json_file(
                std::path::PathBuf::from(path),
            )?),
        })
    }
}

pub fn run() -> sc_cli::Result<()> {
    let cli = Cli::parse();
    let prism_prune = cli.prism_prune;
    let prism_k_prune = cli.prism_k_prune;
    let prism_stage2 = cli.prism_stage2;

    match &cli.subcommand {
        Some(Subcommand::Key(cmd)) => cmd.run(&cli),
        Some(Subcommand::FalconKey(cmd)) => cmd.run().map_err(sc_cli::Error::Input),
        Some(Subcommand::FalconSign(cmd)) => cmd.run().map_err(sc_cli::Error::Input),
        Some(Subcommand::BuildSpec(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.sync_run(|config| cmd.run(config.chain_spec, config.network))
        },
        Some(Subcommand::CheckBlock(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents { client, task_manager, import_queue, .. } =
                    service::new_partial(&config)?;
                Ok((cmd.run(client, import_queue), task_manager))
            })
        },
        Some(Subcommand::ExportBlocks(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents { client, task_manager, .. } = service::new_partial(&config)?;
                Ok((cmd.run(client, config.database), task_manager))
            })
        },
        Some(Subcommand::ExportState(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents { client, task_manager, .. } = service::new_partial(&config)?;
                Ok((cmd.run(client, config.chain_spec), task_manager))
            })
        },
        Some(Subcommand::ImportBlocks(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents { client, task_manager, import_queue, .. } =
                    service::new_partial(&config)?;
                Ok((cmd.run(client, import_queue), task_manager))
            })
        },
        Some(Subcommand::PurgeChain(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.sync_run(|config| cmd.run(config.database))
        },
        Some(Subcommand::Revert(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents { client, task_manager, backend, .. } =
                    service::new_partial(&config)?;
                // No finality gadget to revert: the Prism bridge (node/src/bridge.rs)
                // imports every block already `finalized: true`, so there's nothing
                // extra beyond what `cmd.run` already handles for the base client.
                Ok((cmd.run(client, backend, None), task_manager))
            })
        },
        Some(Subcommand::Benchmark(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.sync_run(|config| {
                match cmd {
                    BenchmarkCmd::Pallet(cmd) => {
                        if !cfg!(feature = "runtime-benchmarks") {
                            return Err("Runtime benchmarking wasn't enabled when building the node. \
                                Use `--features runtime-benchmarks`.".into());
                        }
                        cmd.run_with_spec::<sp_runtime::traits::HashingFor<Block>, ()>(Some(
                            config.chain_spec,
                        ))
                    },
                    BenchmarkCmd::Block(cmd) => {
                        let PartialComponents { client, .. } = service::new_partial(&config)?;
                        cmd.run(client)
                    },
                    #[cfg(not(feature = "runtime-benchmarks"))]
                    BenchmarkCmd::Storage(_) => Err(
                        "Storage benchmarking can be enabled with `--features runtime-benchmarks`."
                            .into(),
                    ),
                    #[cfg(feature = "runtime-benchmarks")]
                    BenchmarkCmd::Storage(cmd) => {
                        let PartialComponents { client, backend, .. } =
                            service::new_partial(&config)?;
                        let db = backend.expose_db();
                        let storage = backend.expose_storage();
                        cmd.run(config, client, db, storage)
                    },
                    #[cfg(not(feature = "dev-signatures"))]
                    BenchmarkCmd::Overhead(_) => Err(
                        "Overhead benchmarking signs extrinsics via sr25519 and needs \
                         `--features dev-signatures`."
                            .into(),
                    ),
                    #[cfg(feature = "dev-signatures")]
                    BenchmarkCmd::Overhead(cmd) => {
                        let PartialComponents { client, .. } = service::new_partial(&config)?;
                        let ext_builder = RemarkBuilder::new(client.clone());
                        cmd.run(
                            config.chain_spec.name().into(),
                            client,
                            inherent_benchmark_data()?,
                            Vec::new(),
                            &ext_builder,
                            false,
                        )
                    },
                    #[cfg(not(feature = "dev-signatures"))]
                    BenchmarkCmd::Extrinsic(_) => Err(
                        "Extrinsic benchmarking signs extrinsics via sr25519 and needs \
                         `--features dev-signatures`."
                            .into(),
                    ),
                    #[cfg(feature = "dev-signatures")]
                    BenchmarkCmd::Extrinsic(cmd) => {
                        let PartialComponents { client, .. } = service::new_partial(&config)?;
                        let ext_factory = ExtrinsicFactory(vec![
                            Box::new(RemarkBuilder::new(client.clone())),
                            Box::new(TransferKeepAliveBuilder::new(
                                client.clone(),
                                Sr25519Keyring::Alice.to_account_id(),
                                EXISTENTIAL_DEPOSIT,
                            )),
                        ]);
                        cmd.run(client, inherent_benchmark_data()?, Vec::new(), &ext_factory)
                    },
                    BenchmarkCmd::Machine(cmd) =>
                        cmd.run(&config, SUBSTRATE_REFERENCE_HARDWARE.clone()),
                }
            })
        },
        #[cfg(feature = "try-runtime")]
        Some(Subcommand::TryRuntime(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let registry = config.prometheus_config.as_ref().map(|cfg| &cfg.registry);
                let task_manager =
                    sc_service::TaskManager::new(config.tokio_handle.clone(), registry)
                        .map_err(|e| sc_cli::Error::Service(sc_service::Error::Prometheus(e)))?;
                Ok((cmd.run::<Block, sp_io::SubstrateHostFunctions>(config), task_manager))
            })
        },
        Some(Subcommand::ChainInfo(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.sync_run(|config| cmd.run::<Block>(&config))
        },
        None => {
            let runner = cli.create_runner(&cli.run)?;
            runner.run_node_until_exit(|config| async move {
                match config.network.network_backend {
                    sc_network::config::NetworkBackendType::Libp2p =>
                        service::new_full::<sc_network::NetworkWorker<
                            Block,
                            <Block as sp_runtime::traits::Block>::Hash,
                        >>(config, prism_prune, prism_k_prune, prism_stage2)
                        .map_err(sc_cli::Error::Service),
                    sc_network::config::NetworkBackendType::Litep2p =>
                        service::new_full::<sc_network::Litep2pNetworkBackend>(
                            config,
                            prism_prune,
                            prism_k_prune,
                            prism_stage2,
                        )
                        .map_err(sc_cli::Error::Service),
                }
            })
        },
    }
}
