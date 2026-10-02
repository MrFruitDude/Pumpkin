/// Returns the Wasmtime [`Config`](wasmtime::Config) every Pumpkin plugin
/// engine starts from.
///
/// The server's plugin host adds its compilation cache on top of this; the
/// P0 mod-loader benchmarks (`pml-bench`) build their engine from the same
/// function so the numbers they report describe the host Pumpkin actually
/// runs, not a hand-copied approximation of it.
#[must_use]
pub fn engine_config() -> wasmtime::Config {
    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    config.concurrency_support(true);
    config.gc_support(true);
    config.wasm_gc(true);
    config.wasm_exceptions(true);
    config.wasm_function_references(true);
    config
}

#[cfg(test)]
mod tests {
    #[test]
    fn engine_config_builds_an_engine() {
        assert!(wasmtime::Engine::new(&super::engine_config()).is_ok());
    }
}
