use crate::pow::{seal_value, Blake3Pow};
use futures::FutureExt;
use argonaut_runtime::{self, opaque::Block, RuntimeApi};
use sc_client_api::Backend;
use sc_consensus_pow::PowBlockImport;
use sc_service::{error::Error as ServiceError, Configuration, TaskManager};
use sc_telemetry::{Telemetry, TelemetryWorker};
use sc_transaction_pool_api::OffchainTransactionPoolFactory;
use std::{sync::Arc, time::Duration};

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

    let net_config = sc_network::config::FullNetworkConfiguration::<
        Block,
        <Block as sp_runtime::traits::Block>::Hash,
        N,
    >::new(&config.network, config.prometheus_registry().cloned());
    let metrics = N::register_notification_metrics(config.prometheus_registry());

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
    }

    network_starter.start_network();
    Ok(task_manager)
}
