//! WASM 技能运行时 —— 安全执行代码型技能
//!
//! 提供沙箱环境执行 WASM 模块，支持资源限制和 Host 函数注入。

use super::{CognitiveError, CognitiveResult, ResourceLimits};

/// WASM 运行时（可选组件）
pub struct WasmRuntime {
    // wasmtime::Engine 需要 wasmtime crate
    // 由于这是可选组件，先提供骨架实现
    enabled: bool,
}

impl WasmRuntime {
    pub fn new() -> CognitiveResult<Self> {
        // TODO: 初始化 wasmtime Engine
        // 需要添加 wasmtime 到 Cargo.toml
        Ok(Self { enabled: false })
    }

    pub fn is_available(&self) -> bool {
        self.enabled
    }

    /// 执行 WASM 模块
    pub async fn execute(
        &self,
        _wasm_bytes: &[u8],
        _inputs: serde_json::Value,
        _limits: &ResourceLimits,
    ) -> CognitiveResult<serde_json::Value> {
        if !self.enabled {
            return Err(CognitiveError::WasmRuntimeNotAvailable);
        }

        // TODO: 实际实现 WASM 执行
        // 1. 编译模块（或从缓存获取）
        // 2. 创建受限 Store
        // 3. 注入 Host 函数
        // 4. 执行并捕获输出

        Ok(serde_json::Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_creates_disabled_runtime() {
        let runtime = WasmRuntime::new().unwrap();
        assert!(!runtime.is_available());
    }

    #[test]
    fn test_is_available_returns_false_for_default() {
        let runtime = WasmRuntime::new().unwrap();
        assert!(!runtime.is_available());
    }

    #[tokio::test]
    async fn test_execute_returns_error_when_not_available() {
        let runtime = WasmRuntime::new().unwrap();
        let limits = ResourceLimits::default();
        let result = runtime.execute(b"", serde_json::Value::Null, &limits).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CognitiveError::WasmRuntimeNotAvailable));
    }
}
