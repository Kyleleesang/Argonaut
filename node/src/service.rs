use crate::pow::{seal_value, Blake3Pow};
use futures::FutureExt;
use argonaut_primitives_prism::Params as PrismParams;
use argonaut_prism_chain::PrismChain;
use argonaut_prism_db::PrismDb;
use argonaut_prism_miner::{build_template, PrismMiner};
use argonaut_prism_network::wire::{blocks_protocol_config, sync_protocol_config, PrismNetworkWorker};
use argonaut_runtime::{self, opaque::Block, RuntimeApi};
use sc_client_api::Backend;
use sc_consensus_pow::PowBlockImport;
use sc_service::{error::Error as ServiceError, Configuration, TaskManager};
use sc_telemetry::{Telemetry, TelemetryWorker};
use sc_transaction_pool_api::OffchainTransactionPoolFactory;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

/// Host function set for the wasm executor.
type HostFunctions =
    (sp_io::SubstrateHostFunctions, argonaut_primitives_falcon::falcon_crypto::HostFunctions);

pub type FullClient =
    sc_service::TFullClient<Block, RuntimeApi, sc_executor::WasmExecutor<HostFunctions>>;
type FullBackend = sc_service::TFullBackend<Block>;
type FullSelectChain = sc_consensus::LongestChain<FullBackend, Block>;
type FullPowBlockImport =
    PowBlockImport<Block, Arc<FullClient>, FullClient, FullSelectChain, Blake3Pow, Box<
        dyn Fn(
                <Block as sp_runtime::traits::Block>::Hash,
                (),
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = Result<
                                sp_timestamp::InherentDataProvider,
                                Box<dyn std::error::Error + Send + Sync>,
                            >,
                        > + Send,
                >,
            > + Send
            + Sync,
    >>;

pub type Service = sc_service::PartialComponents<
    FullClient,
    FullBackend,
    FullSelectChain,
    sc_consensus::DefaultImportQueue<Block>,
    sc_transaction_pool::TransactionPoolHandle<Block, FullClient>,
    (FullPowBlockImport, Option<Telemetry>),
>;

pub fn new_partial(config: &Configuration) -> Result<Service, ServiceError> {
    let telemetry = config
        .telemetry_endpoints
        .clone()
        .filter(|x| !x.is_empty())
        .map(|endpoints| -> Result<_, sc_telemetry::Error> {
            let worker = TelemetryWorker::new(16)?;
            let telemetry = worker.handle().new_telemetry(endpoints);
            Ok((worker, telemetry))
        })
        .transpose()?;

    let executor = sc_service::new_wasm_executor::<HostFunctions>(&config.executor);
    let (client, backend, keystore_container, task_manager) =
        sc_service::new_full_parts::<Block, RuntimeApi, _>(
            config,
            telemetry.as_ref().map(|(_, telemetry)| telemetry.handle()),
            executor,
        )?;
    let client = Arc::new(client);

    let telemetry = telemetry.map(|(worker, telemetry)| {
        task_manager.spawn_handle().spawn("telemetry", None, worker.run());
        telemetry
    });

    let select_chain = sc_consensus::LongestChain::new(backend.clone());

    let transaction_pool = Arc::from(
        sc_transaction_pool::Builder::new(
            task_manager.spawn_essential_handle(),
            client.clone(),
            config.role.is_authority().into(),
        )
        .with_options(config.transaction_pool.clone())
        .with_prometheus(config.prometheus_registry())
        .build(),
    );

    let pow_block_import = PowBlockImport::new(
        client.clone(),
        client.clone(),
        Blake3Pow,
        0,
        select_chain.clone(),
        Box::new(|_, ()| -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            sp_timestamp::InherentDataProvider,
                            Box<dyn std::error::Error + Send + Sync>,
                        >,
                    > + Send,
            >,
        > {
            Box::pin(async move {
                Ok(sp_timestamp::InherentDataProvider::from_system_time())
            })
        }) as _,
    );

    let import_queue = sc_consensus_pow::import_queue(
        Box::new(pow_block_import.clone()),
        None,
        Blake3Pow,
        &task_manager.spawn_essential_handle(),
        config.prometheus_registry(),
    )?;

    Ok(sc_service::PartialComponents {
        client,
        backend,
        task_manager,
        import_queue,
        keystore_container,
        select_chain,
        transaction_pool,
        other: (pow_block_import, telemetry),
    })
}

/// Builds a new service for a full client.
pub fn new_full<N: sc_network::NetworkBackend<Block, <Block as sp_runtime::traits::Block>::Hash>>(
    config: Configuration,
) -> Result<TaskManager, ServiceError> {
    let sc_service::PartialComponents {
        client,
        backend,
        mut task_manager,
        import_queue,
        keystore_container,
        select_chain,
        transaction_pool,
        other: (pow_block_import, mut telemetry),
    } = new_partial(&config)?;

    let mut net_config = sc_network::config::FullNetworkConfiguration::<
        Block,
        <Block as sp_runtime::traits::Block>::Hash,
        N,
    >::new(&config.network, config.prometheus_registry().cloned());
    let metrics = N::register_notification_metrics(config.prometheus_registry());

    // B2: Prism's own gossip (blocks) and sync (orphan/catch-up fetch) protocols,
    // registered alongside the main chain's sync protocol. Config construction
    // must happen before `build_network` consumes `net_config` by value below.
    let (prism_blocks_config, prism_notification_service) =
        blocks_protocol_config::<Block, N>(metrics.clone(), net_config.peer_store_handle());
    net_config.add_notification_protocol(prism_blocks_config);

    let (prism_sync_tx, prism_sync_rx) = async_channel::bounded(64);
    let prism_sync_config = sync_protocol_config::<Block, N>(prism_sync_tx);
    net_config.add_request_response_protocol(prism_sync_config);

    let (network, system_rpc_tx, tx_handler_controller, network_starter, sync_service) =
        sc_service::build_network(sc_service::BuildNetworkParams {
            config: &config,
            net_config,
            client: client.clone(),
            transaction_pool: transaction_pool.clone(),
            spawn_handle: task_manager.spawn_handle(),
            import_queue,
            block_announce_validator_builder: None,
            // No warp sync: that mechanism piggybacks on GRANDPA's authority-set
            // proofs, which this B0 rehearsal chain doesn't have. Full sync is fine
            // for a devnet.
            warp_sync_config: None,
            block_relay: None,
            metrics,
        })?;

    // B2: Prism runs entirely alongside the B0 PoW chain above -- separate DB,
    // separate DAG state machine, separate gossip/sync protocols. It doesn't
    // yet drive the actual Substrate chain (that's B3's job); this is purely
    // the miner+network rehearsal the architecture guide's B2 milestone asks for.
    //
    // Deliberately NOT `Params::testnet()`: at m=100 voter chains, testnet's
    // difficulty (calibrated for a single-target PoW chain, see node/src/pow.rs)
    // produces several *dozen* Prism blocks per second on one CPU, since any of
    // the 102 slots succeeding counts as a find. Empirically that flood of
    // notifications was enough to trip peer reputation/rate-limiting and drop
    // the connection within a couple of seconds during live two-node testing.
    // Scaled down ~100x (roughly `log2(102)` extra bits) to land in the same
    // single-digit-second range as the tuned B0 PoW target.
    let prism_params = {
        let mut p = PrismParams::testnet();
        p.difficulty = p.difficulty >> 7;
        p.f_v = p.f_v >> 7;
        p.f_t = p.f_t >> 7;
        p.f_p = p.f_p >> 7;
        p
    };
    let prism_db = Arc::new(
        PrismDb::open(&config.base_path.path().join("prism"))
            .map_err(|e| ServiceError::Other(format!("failed to open PrismDb: {e:?}")))?,
    );
    let prism_chain = Arc::new(Mutex::new(PrismChain::with_db(prism_params.clone(), prism_db.clone())));

    let (prism_worker, prism_mined_tx) = PrismNetworkWorker::new(
        prism_notification_service,
        prism_sync_rx,
        network.clone(),
        prism_chain.clone(),
        prism_db,
        prism_params.clone(),
    );
    task_manager.spawn_handle().spawn("prism-network", Some("prism"), prism_worker.run());

    if config.offchain_worker.enabled {
        let offchain_workers =
            sc_offchain::OffchainWorkers::new(sc_offchain::OffchainWorkerOptions {
                runtime_api_provider: client.clone(),
                is_validator: config.role.is_authority(),
                keystore: Some(keystore_container.keystore()),
                offchain_db: backend.offchain_storage(),
                transaction_pool: Some(OffchainTransactionPoolFactory::new(
                    transaction_pool.clone(),
                )),
                network_provider: Arc::new(network.clone()),
                enable_http_requests: true,
                custom_extensions: |_| vec![],
            })?;
        task_manager.spawn_handle().spawn(
            "offchain-workers-runner",
            "offchain-worker",
            offchain_workers.run(client.clone(), task_manager.spawn_handle()).boxed(),
        );
    }

    let role = config.role;
    let prometheus_registry = config.prometheus_registry().cloned();

    let rpc_extensions_builder = {
        let client = client.clone();
        let pool = transaction_pool.clone();
        Box::new(move |_| {
            let deps = crate::rpc::FullDeps { client: client.clone(), pool: pool.clone() };
            crate::rpc::create_full(deps).map_err(Into::into)
        })
    };

    let _rpc_handlers = sc_service::spawn_tasks(sc_service::SpawnTasksParams {
        network: Arc::new(network.clone()),
        client: client.clone(),
        keystore: keystore_container.keystore(),
        task_manager: &mut task_manager,
        transaction_pool: transaction_pool.clone(),
        rpc_builder: rpc_extensions_builder,
        backend,
        system_rpc_tx,
        tx_handler_controller,
        sync_service: sync_service.clone(),
        config,
        telemetry: telemetry.as_mut(),
    })?;

    if role.is_authority() {
        let proposer_factory = sc_basic_authorship::ProposerFactory::new(
            task_manager.spawn_handle(),
            client.clone(),
            transaction_pool.clone(),
            prometheus_registry.as_ref(),
            telemetry.as_ref().map(|x| x.handle()),
        );

        let (worker, worker_task) = sc_consensus_pow::start_mining_worker(
            Box::new(pow_block_import),
            client.clone(),
            select_chain,
            Blake3Pow,
            proposer_factory,
            sync_service.clone(),
            sync_service.clone(),
            None,
            move |_, ()| async move {
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                    sp_timestamp::InherentDataProvider::from_system_time(),
                )
            },
            Duration::from_secs(10),
            Duration::from_secs(5),
        );

        task_manager
            .spawn_essential_handle()
            .spawn_blocking("pow-mining-worker", Some("block-authoring"), worker_task);

        // The CPU-bound nonce search itself. Runs on a plain OS thread rather than
        // through `task_manager`'s spawn_blocking: `MiningHandle::submit` holds a
        // `parking_lot` mutex guard (`!Send`) across an `.await` internally, so its
        // returned future isn't `Send` and can't satisfy spawn_blocking's bound.
        // `block_on` sidesteps that -- it only requires the *thread* to block, not
        // the future to be `Send`.
        std::thread::spawn(move || {
            // Keeps climbing across outer-loop iterations (never reset to 0):
            // resetting per-iteration would re-search the exact same nonce range
            // forever whenever a batch happens not to contain a solution.
            let mut nonce: u64 = 0;
            loop {
                let Some(metadata) = worker.metadata() else {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                };
                let version = worker.version();

                let mut found = None;
                for _ in 0..200_000u64 {
                    if seal_value(metadata.pre_hash.as_ref(), nonce) <= metadata.difficulty {
                        found = Some(nonce);
                        nonce = nonce.wrapping_add(1);
                        break;
                    }
                    nonce = nonce.wrapping_add(1);
                }

                if let Some(found_nonce) = found {
                    if worker.version() == version {
                        let seal = found_nonce.to_le_bytes().to_vec();
                        futures::executor::block_on(worker.submit(seal));
                    }
                }
            }
        });

        // B2: Prism mining, entirely separate from the PoW mining loop above --
        // no mempool wired up yet (transaction pool integration is out of scope
        // for this rehearsal), so every template's tx slot is empty. Mined
        // blocks go to `prism_mined_tx`, which `PrismNetworkWorker::run` (spawned
        // above) picks up, inserts into `prism_chain`, and gossips to peers.
        let prism_miner = Arc::new(PrismMiner::new(prism_params.clone()));
        prism_miner.clone().spawn_mining_thread(0, move |block| {
            let _ = prism_mined_tx.unbounded_send(block);
        });

        // Keeps the miner's template in sync with the DAG: since there's no
        // mempool yet, only the parent/vote/reference slots ever change.
        std::thread::spawn(move || {
            let mut queue: u64 = 0;
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let tmpl = {
                    let chain = prism_chain.lock().expect("PrismChain mutex poisoned");
                    build_template(&chain, &[], queue, 8, &prism_params)
                };
                prism_miner.refresh(tmpl);
                queue = (queue + 1) % 8;
            }
        });
    }

    network_starter.start_network();
    Ok(task_manager)
}
