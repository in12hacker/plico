//! WASM 技能运行时 —— 安全执行代码型技能
//!
//! 提供沙箱环境执行 WASM 模块，支持资源限制和 Host 函数注入。
//! 使用 wasmtime 作为后端（feature-gated）。

use super::{CognitiveError, CognitiveResult, ResourceLimits, ToolExecutor};

// ── Real wasmtime implementation ──────────────────────────────────────

#[cfg(feature = "wasmtime-backend")]
mod wasmtime_impl {
    use super::*;
    use std::collections::HashMap;
    use std::sync::RwLock;

    use wasmtime::*;

    /// Host state passed through Store to host functions
    struct HostState {
        executor: Option<std::sync::Arc<dyn ToolExecutor>>,
    }

    /// WASM 运行时（wasmtime 后端）
    pub struct WasmRuntime {
        engine: Engine,
        cache: RwLock<HashMap<[u8; 32], Module>>,
    }

    impl WasmRuntime {
        pub fn new() -> CognitiveResult<Self> {
            let mut config = Config::new();
            config.consume_fuel(true);

            let engine = Engine::new(&config).map_err(|e| CognitiveError::WasmInitFailed(e.to_string()))?;

            Ok(Self {
                engine,
                cache: RwLock::new(HashMap::new()),
            })
        }

        pub fn is_available(&self) -> bool {
            true
        }

        /// 执行 WASM 模块
        pub async fn execute(
            &self,
            wasm_bytes: &[u8],
            inputs: serde_json::Value,
            limits: &ResourceLimits,
            executor: Option<std::sync::Arc<dyn ToolExecutor>>,
        ) -> CognitiveResult<serde_json::Value> {
            // 1. Compile or retrieve from cache
            let module = self.get_or_compile(wasm_bytes)?;

            // 2. Create store with fuel limit and host state
            let fuel_limit = limits.max_execution_time_ms * 1000; // ms → fuel units
            let host_state = HostState { executor };
            let mut store = Store::new(&self.engine, host_state);
            store
                .set_fuel(fuel_limit)
                .map_err(|e| CognitiveError::WasmExecutionFailed(format!("Failed to set fuel: {}", e)))?;

            // 3. Set memory limits
            let memory_ty = MemoryType::new(limits.max_memory_pages(), Some(limits.max_memory_pages()));
            let memory = Memory::new(&mut store, memory_ty)
                .map_err(|e| CognitiveError::WasmExecutionFailed(format!("Failed to create memory: {}", e)))?;

            // 4. Build host functions
            let log_func = Func::wrap(&mut store, |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
                if let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) {
                    let data = memory.data(&caller);
                    let start = ptr as usize;
                    let end = start + len as usize;
                    if end <= data.len() {
                        if let Ok(msg) = std::str::from_utf8(&data[start..end]) {
                            tracing::debug!("[WASM] {}", msg);
                        }
                    }
                }
            });

            let tool_call_func = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, HostState>,
                 name_ptr: i32,
                 name_len: i32,
                 params_ptr: i32,
                 params_len: i32,
                 result_ptr: i32,
                 result_capacity: i32|
                 -> i32 {
                    // Read tool name from WASM memory
                    let name = {
                        let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                            Some(m) => m,
                            None => return -1,
                        };
                        let data = memory.data(&caller);
                        let start = name_ptr as usize;
                        let end = start + name_len as usize;
                        if end > data.len() {
                            return -2;
                        }
                        match std::str::from_utf8(&data[start..end]) {
                            Ok(s) => s.to_string(),
                            Err(_) => return -3,
                        }
                    };

                    // Read params from WASM memory
                    let params: serde_json::Value = {
                        let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                            Some(m) => m,
                            None => return -1,
                        };
                        let data = memory.data(&caller);
                        let start = params_ptr as usize;
                        let end = start + params_len as usize;
                        if end > data.len() {
                            return -2;
                        }
                        match std::str::from_utf8(&data[start..end]) {
                            Ok(s) => serde_json::from_str(s).unwrap_or(serde_json::Value::Null),
                            Err(_) => return -3,
                        }
                    };

                    // Execute tool via executor
                    let state = caller.data();
                    let executor = match state.executor.as_ref() {
                        Some(e) => e,
                        None => return -4, // no executor available
                    };

                    let result = match executor.execute_tool(&name, &params, "wasm") {
                        Ok(val) => val,
                        Err(_) => return -5,
                    };

                    // Write result back to WASM memory
                    let result_bytes = serde_json::to_vec(&result).unwrap_or_default();
                    let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                        Some(m) => m,
                        None => return -1,
                    };
                    let rptr = result_ptr as usize;
                    let cap = result_capacity as usize;
                    if rptr + 4 + result_bytes.len() > memory.data(&caller).len() || result_bytes.len() > cap {
                        return -6; // buffer too small
                    }
                    let len_bytes = (result_bytes.len() as i32).to_le_bytes();
                    memory.data_mut(&mut caller)[rptr..rptr + 4].copy_from_slice(&len_bytes);
                    memory.data_mut(&mut caller)[rptr + 4..rptr + 4 + result_bytes.len()]
                        .copy_from_slice(&result_bytes);

                    result_bytes.len() as i32
                },
            );

            // 5. Create linker and define host functions + memory
            let mut linker = Linker::new(&self.engine);
            linker
                .define(&mut store, "env", "plico_log", log_func)
                .map_err(|e| CognitiveError::WasmExecutionFailed(format!("Failed to define plico_log: {}", e)))?;
            linker
                .define(&mut store, "env", "plico_tool_call", tool_call_func)
                .map_err(|e| CognitiveError::WasmExecutionFailed(format!("Failed to define plico_tool_call: {}", e)))?;
            linker
                .define(&mut store, "env", "memory", memory)
                .map_err(|e| CognitiveError::WasmExecutionFailed(format!("Failed to define memory: {}", e)))?;

            // 6. Serialize inputs
            let inputs_json = serde_json::to_string(&inputs)
                .map_err(|e| CognitiveError::WasmExecutionFailed(format!("Failed to serialize inputs: {}", e)))?;

            // 7. Instantiate
            let instance = linker
                .instantiate(&mut store, &module)
                .map_err(|e| CognitiveError::WasmExecutionFailed(format!("Instantiation failed: {}", e)))?;

            // 8. Call main
            let main_func = instance
                .get_func(&mut store, "main")
                .ok_or_else(|| CognitiveError::WasmExecutionFailed("Module has no 'main' function".to_string()))?;

            // Write inputs to memory before calling main
            if let Some(memory) = instance.get_memory(&mut store, "memory") {
                let input_bytes = inputs_json.as_bytes();
                let input_len = input_bytes.len() as i32;
                memory.data_mut(&mut store)[..4].copy_from_slice(&input_len.to_le_bytes());
                let data_start = 4;
                let data_end = data_start + input_bytes.len();
                if data_end <= memory.data(&store).len() {
                    memory.data_mut(&mut store)[data_start..data_end].copy_from_slice(input_bytes);
                }
            }

            let input_ptr: i32 = 0;
            let input_len: i32 = inputs_json.len() as i32 + 4;
            let mut results = [Val::I32(0)];
            main_func
                .call(&mut store, &[Val::I32(input_ptr), Val::I32(input_len)], &mut results)
                .map_err(|e| {
                    if e.to_string().contains("fuel") {
                        CognitiveError::WasmExecutionFailed("Execution exceeded fuel limit (timeout)".to_string())
                    } else {
                        CognitiveError::WasmExecutionFailed(format!("Execution failed: {}", e))
                    }
                })?;

            // 9. Read output from return value or memory
            let output_ptr = match results[0] {
                Val::I32(ptr) => ptr as usize,
                _ => return Ok(serde_json::Value::Null),
            };

            if let Some(memory) = instance.get_memory(&mut store, "memory") {
                let data = memory.data(&store);
                if output_ptr + 4 <= data.len() {
                    let len = i32::from_le_bytes([
                        data[output_ptr],
                        data[output_ptr + 1],
                        data[output_ptr + 2],
                        data[output_ptr + 3],
                    ]) as usize;
                    let start = output_ptr + 4;
                    let end = start + len;
                    if end <= data.len() {
                        if let Ok(json_str) = std::str::from_utf8(&data[start..end]) {
                            if let Ok(value) = serde_json::from_str(json_str) {
                                return Ok(value);
                            }
                        }
                    }
                }
            }

            Ok(serde_json::json!({ "result": results[0].unwrap_i32() }))
        }

        /// 仅编译 WASM 模块（不执行），用于验证字节码有效性
        pub fn compile_only(&self, wasm_bytes: &[u8]) -> CognitiveResult<()> {
            self.get_or_compile(wasm_bytes)?;
            Ok(())
        }

        fn get_or_compile(&self, wasm_bytes: &[u8]) -> CognitiveResult<Module> {
            let hash = sha256_bytes(wasm_bytes);

            // Check cache
            {
                let cache = self
                    .cache
                    .read()
                    .map_err(|e| CognitiveError::WasmInitFailed(format!("Cache lock poisoned: {}", e)))?;
                if let Some(module) = cache.get(&hash) {
                    return Ok(module.clone());
                }
            }

            // Compile
            let module = Module::new(&self.engine, wasm_bytes)
                .map_err(|e| CognitiveError::WasmInitFailed(format!("Compilation failed: {}", e)))?;

            // Cache
            {
                let mut cache = self
                    .cache
                    .write()
                    .map_err(|e| CognitiveError::WasmInitFailed(format!("Cache lock poisoned: {}", e)))?;
                cache.insert(hash, module.clone());
            }

            Ok(module)
        }
    }

    fn sha256_bytes(data: &[u8]) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher.finalize().into()
    }
}

// ── Stub implementation (when wasmtime feature is disabled) ───────────

#[cfg(not(feature = "wasmtime-backend"))]
mod stub_impl {
    use super::*;

    /// WASM 运行时（桩实现 — wasmtime feature 未启用）
    pub struct WasmRuntime {
        enabled: bool,
    }

    impl WasmRuntime {
        pub fn new() -> CognitiveResult<Self> {
            Ok(Self { enabled: false })
        }

        pub fn is_available(&self) -> bool {
            self.enabled
        }

        pub async fn execute(
            &self,
            _wasm_bytes: &[u8],
            _inputs: serde_json::Value,
            _limits: &ResourceLimits,
            _executor: Option<std::sync::Arc<dyn ToolExecutor>>,
        ) -> CognitiveResult<serde_json::Value> {
            if !self.enabled {
                return Err(CognitiveError::WasmRuntimeNotAvailable);
            }
            Ok(serde_json::Value::Null)
        }

        pub fn compile_only(&self, _wasm_bytes: &[u8]) -> CognitiveResult<()> {
            if !self.enabled {
                return Err(CognitiveError::WasmRuntimeNotAvailable);
            }
            Ok(())
        }
    }
}

// ── Re-export based on feature ────────────────────────────────────────

#[cfg(feature = "wasmtime-backend")]
pub use wasmtime_impl::WasmRuntime;

#[cfg(not(feature = "wasmtime-backend"))]
pub use stub_impl::WasmRuntime;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_creates_runtime() {
        let runtime = WasmRuntime::new().unwrap();
        // With wasmtime-backend: available; without: not available
        #[cfg(feature = "wasmtime-backend")]
        assert!(runtime.is_available());
        #[cfg(not(feature = "wasmtime-backend"))]
        assert!(!runtime.is_available());
    }

    #[tokio::test]
    async fn test_execute_returns_error_when_not_available() {
        let runtime = WasmRuntime::new().unwrap();
        let limits = ResourceLimits::default();
        if !runtime.is_available() {
            let result = runtime.execute(b"", serde_json::Value::Null, &limits, None).await;
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), CognitiveError::WasmRuntimeNotAvailable));
        }
    }

    #[test]
    fn test_compile_only_returns_error_when_not_available() {
        let runtime = WasmRuntime::new().unwrap();
        if !runtime.is_available() {
            let result = runtime.compile_only(b"");
            assert!(result.is_err());
        }
    }

    #[cfg(feature = "wasmtime-backend")]
    mod wasmtime_tests {
        use super::*;

        fn minimal_wasm() -> Vec<u8> {
            wat::parse_str(
                r#"
                (module
                    (memory (export "memory") 1)
                    (func (export "main") (param i32 i32) (result i32)
                        i32.const 0)
                )
            "#,
            )
            .unwrap()
        }

        fn infinite_loop_wasm() -> Vec<u8> {
            // Function with many iterations to exhaust fuel
            wat::parse_str(
                r#"
                (module
                    (memory (export "memory") 1)
                    (func (export "main") (param i32 i32) (result i32)
                        (local i64)
                        (local.set 2 (i64.const 1000000000))
                        (block $break
                            (loop $continue
                                (br_if $break (i64.eqz (local.get 2)))
                                (local.set 2 (i64.sub (local.get 2) (i64.const 1)))
                                (br $continue)))
                        i32.const 0)
                )
            "#,
            )
            .unwrap()
        }

        #[tokio::test]
        async fn test_execute_minimal_module() {
            let runtime = WasmRuntime::new().unwrap();
            let limits = ResourceLimits::default();
            let result = runtime
                .execute(&minimal_wasm(), serde_json::json!({}), &limits, None)
                .await;
            assert!(result.is_ok(), "execute failed: {:?}", result.err());
        }

        #[tokio::test]
        async fn test_compile_error_returns_init_failed() {
            let runtime = WasmRuntime::new().unwrap();
            let limits = ResourceLimits::default();
            let result = runtime.execute(b"not wasm", serde_json::json!({}), &limits, None).await;
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), CognitiveError::WasmInitFailed(_)));
        }

        #[tokio::test]
        async fn test_module_caching() {
            let runtime = WasmRuntime::new().unwrap();
            let limits = ResourceLimits::default();
            let wasm = minimal_wasm();
            let r1 = runtime.execute(&wasm, serde_json::json!({}), &limits, None).await;
            let r2 = runtime.execute(&wasm, serde_json::json!({}), &limits, None).await;
            assert!(r1.is_ok());
            assert!(r2.is_ok());
        }

        #[tokio::test]
        async fn test_fuel_exhaustion() {
            let runtime = WasmRuntime::new().unwrap();
            let limits = ResourceLimits {
                max_execution_time_ms: 1, // very low fuel
                ..ResourceLimits::default()
            };
            let result = runtime
                .execute(&infinite_loop_wasm(), serde_json::json!({}), &limits, None)
                .await;
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), CognitiveError::WasmExecutionFailed(_)));
        }

        #[test]
        fn test_compile_only_valid_wasm() {
            let runtime = WasmRuntime::new().unwrap();
            let result = runtime.compile_only(&minimal_wasm());
            assert!(result.is_ok());
        }

        #[test]
        fn test_compile_only_invalid_wasm() {
            let runtime = WasmRuntime::new().unwrap();
            let result = runtime.compile_only(b"not wasm");
            assert!(result.is_err());
        }
    }
}
