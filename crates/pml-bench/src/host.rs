//! The benchmark guest and the two ways the host calls into it.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

use pumpkin_plugin_runtime::{
    LegacyStore, LegacySyncReentry, RuntimeSpawner, SpawnError, SpawnFuture,
};
use wasmtime::{
    Engine, Store, UpdateDeadline,
    component::{Component, Linker, TypedFunc},
};

/// The benchmark guest component, in the component-model text format.
pub const GUEST_WAT: &str = include_str!("guest.wat");

/// Epochs the deadline is pushed out by each time it is reached. With a 1 ms
/// ticker this is the 5 ms hard cap per sync hook planned in spec §6.3.
pub const EPOCH_DEADLINE_TICKS: u64 = 5;

/// How often the epoch ticker advances the engine epoch (spec §2.3: 1 ms).
pub const EPOCH_TICK: Duration = Duration::from_millis(1);

/// Arguments of the block-hook-shaped export: hook id, block position, state.
pub type HookArgs = (u32, i32, i32, i32, u32);

/// Store data for the benchmark guest.
#[derive(Default)]
pub struct HostState {
    /// Number of guest-to-host calls served, so tests can prove the imports
    /// really were reached.
    pub host_calls: u64,
}

/// Whether the engine compiles epoch-interruption checks into guest code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EpochMode {
    /// Pumpkin's host today: no epoch checks.
    Off,
    /// What spec §6.3 plans: epoch checks plus a 1 ms ticker thread, with a
    /// deadline that is extended (not trapped) so the benchmark keeps running.
    On,
    /// Epoch checks with a deadline that traps, to verify enforcement works.
    Trap,
}

/// How the guest's imports are bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Imports {
    /// As on Pumpkin's host: `host-noop-async` is an async host function, so
    /// Wasmtime only allows `call_async` (each call runs on a fiber stack).
    Pumpkin,
    /// Every import is a plain sync host function, which lets the host use
    /// the synchronous `call` with no fiber switch. A diagnostic variant: it
    /// is not what Pumpkin's host does today.
    SyncOnly,
}

/// Compiles the benchmark guest into a component for `engine`.
///
/// # Errors
/// When the guest text does not parse or does not validate on `engine`.
pub fn guest_component(engine: &Engine) -> wasmtime::Result<Component> {
    let bytes = wat::parse_str(GUEST_WAT).map_err(wasmtime::Error::msg)?;
    Component::new(engine, bytes)
}

/// Which Wasmtime configuration the engine starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineFlavor {
    /// [`pumpkin_plugin_runtime::engine_config`], what the server runs.
    Pumpkin,
    /// The component model and nothing else: no async, no concurrency, no
    /// GC. A diagnostic floor for what a component call costs in this
    /// Wasmtime revision at all. Use with [`Imports::SyncOnly`] so the
    /// synchronous `call` is allowed.
    ComponentModelOnly,
}

/// Builds an engine from Pumpkin's plugin engine configuration.
///
/// # Errors
/// When Wasmtime rejects the configuration on this host.
pub fn engine(epoch: EpochMode) -> wasmtime::Result<Engine> {
    engine_with(EngineFlavor::Pumpkin, epoch)
}

/// Builds an engine of the given flavor.
///
/// # Errors
/// When Wasmtime rejects the configuration on this host.
pub fn engine_with(flavor: EngineFlavor, epoch: EpochMode) -> wasmtime::Result<Engine> {
    let mut config = match flavor {
        EngineFlavor::Pumpkin => pumpkin_plugin_runtime::engine_config(),
        EngineFlavor::ComponentModelOnly => {
            let mut config = wasmtime::Config::new();
            config.wasm_component_model(true);
            config
        }
    };
    if epoch != EpochMode::Off {
        config.epoch_interruption(true);
    }
    Engine::new(&config)
}

fn linker(engine: &Engine, imports: Imports) -> wasmtime::Result<Linker<HostState>> {
    let mut linker = Linker::<HostState>::new(engine);
    match imports {
        // Pumpkin's generated host bindings bind imports as `async | trappable`,
        // which Wasmtime links through `func_wrap_async`.
        Imports::Pumpkin => {
            linker
                .root()
                .func_wrap_async("host-noop-async", |mut store, (x,): (u32,)| {
                    store.data_mut().host_calls += 1;
                    Box::new(async move { Ok((x.wrapping_add(1),)) })
                })?;
        }
        Imports::SyncOnly => {
            linker
                .root()
                .func_wrap("host-noop-async", |mut store, (x,): (u32,)| {
                    store.data_mut().host_calls += 1;
                    Ok((x.wrapping_add(1),))
                })?;
        }
    }
    linker
        .root()
        .func_wrap("host-noop-sync", |mut store, (x,): (u32,)| {
            store.data_mut().host_calls += 1;
            Ok((x.wrapping_add(1),))
        })?;
    Ok(linker)
}

/// Advances an engine's epoch every [`EPOCH_TICK`] until dropped.
struct EpochTicker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl EpochTicker {
    fn start(engine: Engine) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("pml-bench-epoch".into())
            .spawn(move || {
                while !stop_for_thread.load(Ordering::Relaxed) {
                    std::thread::sleep(EPOCH_TICK);
                    engine.increment_epoch();
                }
            })
            .ok();
        Self { stop, thread }
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone, Copy)]
struct Exports {
    hook: TypedFunc<HookArgs, (u32,)>,
    call_host_async: TypedFunc<(u32,), (u32,)>,
    call_host_sync: TypedFunc<(u32,), (u32,)>,
    list_len: TypedFunc<(Vec<u8>,), (u32,)>,
    tick_batch: TypedFunc<(Vec<u64>,), (u64,)>,
    spin: TypedFunc<(u32,), (u32,)>,
}

/// The guest instantiated on a Store the caller drives directly, with no
/// executor in between. This is the floor any dispatch path can reach.
pub struct BenchGuest {
    store: Store<HostState>,
    exports: Exports,
    ticker: Option<EpochTicker>,
}

impl BenchGuest {
    /// Compiles and instantiates the guest on a fresh Pumpkin-configured engine.
    ///
    /// # Errors
    /// When compilation, linking or instantiation fails.
    pub async fn new(epoch: EpochMode) -> wasmtime::Result<Self> {
        Self::with_imports(epoch, Imports::Pumpkin).await
    }

    /// Like [`Self::new`], choosing how imports are bound.
    ///
    /// # Errors
    /// When compilation, linking or instantiation fails.
    pub async fn with_imports(epoch: EpochMode, imports: Imports) -> wasmtime::Result<Self> {
        Self::with_options(EngineFlavor::Pumpkin, epoch, imports).await
    }

    /// Fully configurable constructor for the diagnostic variants.
    ///
    /// # Errors
    /// When compilation, linking or instantiation fails.
    pub async fn with_options(
        flavor: EngineFlavor,
        epoch: EpochMode,
        imports: Imports,
    ) -> wasmtime::Result<Self> {
        let engine = engine_with(flavor, epoch)?;
        let component = guest_component(&engine)?;
        let linker = linker(&engine, imports)?;
        let mut store = Store::new(&engine, HostState::default());
        match epoch {
            EpochMode::Off => {}
            EpochMode::On => {
                store.set_epoch_deadline(EPOCH_DEADLINE_TICKS);
                store.epoch_deadline_callback(|_| {
                    Ok(UpdateDeadline::Continue(EPOCH_DEADLINE_TICKS))
                });
            }
            EpochMode::Trap => {
                store.set_epoch_deadline(EPOCH_DEADLINE_TICKS);
                store.epoch_deadline_trap();
            }
        }
        let instance = linker.instantiate_async(&mut store, &component).await?;
        let exports = Exports {
            hook: instance.get_typed_func(&mut store, "hook")?,
            call_host_async: instance.get_typed_func(&mut store, "call-host-async")?,
            call_host_sync: instance.get_typed_func(&mut store, "call-host-sync")?,
            list_len: instance.get_typed_func(&mut store, "list-len")?,
            tick_batch: instance.get_typed_func(&mut store, "tick-batch")?,
            spin: instance.get_typed_func(&mut store, "spin")?,
        };
        let ticker = (epoch != EpochMode::Off).then(|| EpochTicker::start(engine));
        Ok(Self {
            store,
            exports,
            ticker,
        })
    }

    /// Guest-to-host calls served so far.
    #[must_use]
    pub fn host_calls(&self) -> u64 {
        self.store.data().host_calls
    }

    /// Host-to-guest block-hook-shaped call.
    ///
    /// # Errors
    /// When the guest traps.
    pub async fn hook(&mut self, args: HookArgs) -> wasmtime::Result<u32> {
        let (result,) = self.exports.hook.call_async(&mut self.store, args).await?;
        Ok(result)
    }

    /// Host-to-guest hook call through the synchronous `call`. Only allowed
    /// with [`Imports::SyncOnly`]; Wasmtime refuses it otherwise.
    ///
    /// # Errors
    /// When the guest traps or the Store requires `call_async`.
    pub fn hook_sync(&mut self, args: HookArgs) -> wasmtime::Result<u32> {
        let (result,) = self.exports.hook.call(&mut self.store, args)?;
        Ok(result)
    }

    /// Makes the guest call the `async`-bound host import `n` times.
    ///
    /// # Errors
    /// When the guest traps.
    pub async fn call_host_async(&mut self, n: u32) -> wasmtime::Result<u32> {
        let (result,) = self
            .exports
            .call_host_async
            .call_async(&mut self.store, (n,))
            .await?;
        Ok(result)
    }

    /// Makes the guest call the synchronously bound host import `n` times.
    ///
    /// # Errors
    /// When the guest traps.
    pub async fn call_host_sync(&mut self, n: u32) -> wasmtime::Result<u32> {
        let (result,) = self
            .exports
            .call_host_sync
            .call_async(&mut self.store, (n,))
            .await?;
        Ok(result)
    }

    /// Copies `data` into guest memory; the guest returns its length.
    ///
    /// # Errors
    /// When the guest traps.
    pub async fn list_len(&mut self, data: Vec<u8>) -> wasmtime::Result<u32> {
        let (result,) = self
            .exports
            .list_len
            .call_async(&mut self.store, (data,))
            .await?;
        Ok(result)
    }

    /// One batched block-entity tick over `handles`; returns the guest's
    /// running sum of every handle it has seen.
    ///
    /// # Errors
    /// When the guest traps.
    pub async fn tick_batch(&mut self, handles: Vec<u64>) -> wasmtime::Result<u64> {
        let (result,) = self
            .exports
            .tick_batch
            .call_async(&mut self.store, (handles,))
            .await?;
        Ok(result)
    }

    /// Runs `n` iterations of a guest-only compute loop.
    ///
    /// # Errors
    /// When the guest traps, e.g. on an epoch deadline in [`EpochMode::Trap`].
    pub async fn spin(&mut self, n: u32) -> wasmtime::Result<u32> {
        let (result,) = self.exports.spin.call_async(&mut self.store, (n,)).await?;
        Ok(result)
    }

    /// Moves the Store behind Pumpkin's plugin store executor, exactly as the
    /// server does after instantiating a plugin.
    ///
    /// # Errors
    /// When the executor's driver task cannot start.
    pub async fn into_executor(
        self,
        runtime: tokio::runtime::Handle,
    ) -> wasmtime::Result<ExecutorGuest> {
        let spawner: Arc<dyn RuntimeSpawner> = Arc::new(TokioSpawner { runtime });
        let executor = LegacyStore::start(self.store, LegacySyncReentry::new(), spawner).await?;
        Ok(ExecutorGuest {
            executor,
            exports: self.exports,
            _ticker: self.ticker,
        })
    }
}

/// Same spawner the server uses (`wasm_host::concurrent_store::TokioSpawner`).
struct TokioSpawner {
    runtime: tokio::runtime::Handle,
}

impl RuntimeSpawner for TokioSpawner {
    fn spawn(&self, task: SpawnFuture) -> Result<(), SpawnError> {
        drop(self.runtime.spawn(task));
        Ok(())
    }

    fn spawn_blocking(&self, task: Box<dyn FnOnce() + Send + 'static>) -> Result<(), SpawnError> {
        drop(self.runtime.spawn_blocking(task));
        Ok(())
    }
}

/// The guest behind [`pumpkin_plugin_runtime::StoreExecutor`]: every call is
/// submitted through `call_guest`, as `WasmPluginEventHandler` does for each
/// plugin event.
pub struct ExecutorGuest {
    executor: LegacyStore<HostState>,
    exports: Exports,
    // Declared after `executor` so the ticker outlives the Store's last call.
    _ticker: Option<EpochTicker>,
}

impl ExecutorGuest {
    /// Host-to-guest block-hook-shaped call through the executor.
    ///
    /// # Errors
    /// When the guest traps or the executor is no longer running.
    pub async fn hook(&self, args: HookArgs) -> wasmtime::Result<u32> {
        let hook = self.exports.hook;
        self.executor
            .call_guest(move |mut guest| {
                Box::pin(async move { guest.call(hook, args).await.map(|(result,)| result) })
            })
            .await
    }

    /// One batched block-entity tick through the executor.
    ///
    /// # Errors
    /// When the guest traps or the executor is no longer running.
    pub async fn tick_batch(&self, handles: Vec<u64>) -> wasmtime::Result<u64> {
        let tick_batch = self.exports.tick_batch;
        self.executor
            .call_guest(move |mut guest| {
                Box::pin(async move {
                    guest
                        .call(tick_batch, (handles,))
                        .await
                        .map(|(result,)| result)
                })
            })
            .await
    }

    /// Guest-to-host calls served so far.
    ///
    /// # Errors
    /// When the executor is no longer running.
    pub async fn host_calls(&self) -> wasmtime::Result<u64> {
        self.executor
            .call(|accessor| {
                Box::pin(
                    async move { Ok(accessor.with(|mut access| access.data_mut().host_calls)) },
                )
            })
            .await
    }

    /// Drains and stops the executor, as plugin unload does.
    ///
    /// # Errors
    /// When the driver failed.
    pub async fn shutdown(self) -> wasmtime::Result<()> {
        self.executor
            .shutdown(|_| Box::pin(async move { Ok(()) }))
            .await
    }
}
