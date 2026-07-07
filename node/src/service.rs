use crate::bridge::{run_prism_bridge, PrismBridgeVerifier};
use futures::FutureExt;
use argonaut_primitives_prism::Params as PrismParams;
use argonaut_prism_chain::PrismChain;
use argonaut_prism_db::PrismDb;
use argonaut_prism_ledger::K_DEEP_CONFIRMATION;
use argonaut_prism_miner::{build_template, PrismMiner};
use argonaut_prism_network::wire::{blocks_protocol_config, sync_protocol_config, PrismNetworkWorker};
use argonaut_runtime::{self, opaque::Block, RuntimeApi};
use parity_scale_codec::Encode;
use sc_client_api::Backend;
use sc_consensus::import_queue::{BasicQueue, BoxBlockImport};
use sc_service::{error::Error as ServiceError, Configuration, TaskManager};
use sc_telemetry::{Telemetry, TelemetryWorker};
use sc_transaction_pool_api::{InPoolTransaction, OffchainTransactionPoolFactory, TransactionPool};
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


pub type Service = sc_service::PartialComponents<
    FullClient,
    FullBackend,
    FullSelectChain,
    sc_consensus::DefaultImportQueue<Block>,
    sc_transaction_pool::TransactionPoolHandle<Block, FullClient>,
    Option<Telemetry>,
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

    // B3: the Prism bridge (`node/src/bridge.rs`) is the only block author --
    // it derives one Substrate block per confirmed Prism leader level and
    // imports it directly (bypassing this queue entirely). This queue only
    // ever sees blocks arriving over the network via ordinary Substrate block
    // sync (e.g. a node catching up after being offline). `PrismBridgeVerifier`
    // marks every such block finalized/always-best, matching how the bridge
    // itself imports self-authored blocks -- the real safety check is the
    // default `StateAction::Execute` re-executing the block against the
    // runtime, same as any other Substrate import.
    let block_import: BoxBlockImport<Block> = Box::new(client.clone());
    let import_queue = BasicQueue::new(
        PrismBridgeVerifier,
        block_import,
        None,
        &task_manager.spawn_essential_handle(),
        config.prometheus_registry(),
    );

    Ok(sc_service::PartialComponents {
        client,
        backend,
        task_manager,
        import_queue,
        keystore_container,
        select_chain,
        transaction_pool,
        other: telemetry,
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
        select_chain: _,
        transaction_pool,
        other: mut telemetry,
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
            // proofs, which this chain doesn't have (Prism is ε-final; see B3's
            // bridge in `node/src/bridge.rs`). Full sync is fine for a devnet.
            warp_sync_config: None,
            block_relay: None,
            metrics,
        })?;

    // B1-B3: Prism runs entirely client-side (architecture guide §3.1) -- its
    // own DAG state machine, gossip/sync protocols, miner, and (B3) the bridge
    // that derives the actual Substrate chain from confirmed leader levels.
    //
    // Deliberately NOT `Params::testnet()` unmodified: at m=100 voter chains,
    // testnet's difficulty (calibrated for a single-target PoW chain) produces
    // several *dozen* Prism blocks per second on one CPU, since any of the 102
    // slots succeeding counts as a find. Empirically that flood of
    // notifications was enough to trip peer reputation/rate-limiting and drop
    // the connection within a couple of seconds during live two-node testing
    // (B2). Scaled down ~100x (roughly `log2(102)` extra bits) to land in a
    // manageable single-digit-second range -- `f_v`/`f_t`/`f_p` are scaled
    // identically alongside `difficulty` so the m=100 slot split is preserved
    // exactly (only `m_voter_chains` itself would need `f_v`/`f_t`/`f_p`
    // rederiving from scratch, per `client/prism-ledger`'s own devnet-tuning
    // notes in `docs/IMPLEMENTATION_B_PROGRESS.md`).
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
        prism_db.clone(),
        prism_params.clone(),
    );
    task_manager.spawn_handle().spawn("prism-network", Some("prism"), prism_worker.run());

    // B3: every full node -- authority or not -- runs the bridge. Prism
    // confirmation is a deterministic function of the DAG every node already
    // observes via gossip above, so every honest node derives byte-identical
    // Substrate blocks independently; this isn't gated on being a block
    // author the way PoW/PoA authorship would be.
    task_manager.spawn_handle().spawn(
        "prism-bridge",
        Some("prism"),
        run_prism_bridge(client.clone(), client.clone(), prism_chain.clone(), prism_db, K_DEEP_CONFIRMATION),
    );

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
        // B2/B3: Prism mining -- unrelated to Substrate block authorship
        // (that's the bridge above). Mined blocks go to `prism_mined_tx`,
        // which `PrismNetworkWorker::run` (spawned above) picks up, inserts
        // into `prism_chain`, and gossips to peers.
        let prism_miner = Arc::new(PrismMiner::new(prism_params.clone()));
        prism_miner.clone().spawn_mining_thread(0, move |block| {
            let _ = prism_mined_tx.unbounded_send(block);
        });

        // Keeps the miner's template in sync with the DAG and the live tx
        // pool. B3: pulls ready extrinsics' encoded bytes from the pool on
        // every refresh -- previously this was always `&[]` (no mempool
        // wired up at all), which meant no user-submitted transaction could
        // ever reach a Prism tx block. `.data().encode()` reproduces the
        // exact SCALE bytes the bridge later decodes back via
        // `OpaqueExtrinsic::from_bytes` (both sides agree on the same wire
        // format because they're the same bytes, just round-tripped).
        let mempool_pool = transaction_pool.clone();
        std::thread::spawn(move || {
            let mut queue: u64 = 0;
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let mempool: Vec<Vec<u8>> =
                    mempool_pool.ready().map(|tx| tx.data().encode()).collect();
                let tmpl = {
                    let chain = prism_chain.lock().expect("PrismChain mutex poisoned");
                    build_template(&chain, &mempool, queue, 8, &prism_params)
                };
                prism_miner.refresh(tmpl);
                queue = (queue + 1) % 8;
            }
        });
    }

    network_starter.start_network();
    Ok(task_manager)
}
