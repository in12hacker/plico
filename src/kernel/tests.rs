//! Test helpers for kernel inline #[cfg(test)] modules.
//!
//! Mirrors the helpers from `tests/kernel_test.rs` so that ops modules
//! can use `make_kernel()` without importing from the test crate.

#[cfg(test)]
pub fn make_kernel() -> (crate::kernel::AIKernel, tempfile::TempDir) {
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
    let dir = tempfile::tempdir().unwrap();
    let kernel = crate::kernel::AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
    (kernel, dir)
}

#[cfg(test)]
pub fn make_kernel_arc() -> (std::sync::Arc<crate::kernel::AIKernel>, tempfile::TempDir) {
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
    let dir = tempfile::tempdir().unwrap();
    let kernel = std::sync::Arc::new(crate::kernel::AIKernel::new(dir.path().to_path_buf()).expect("kernel init"));
    (kernel, dir)
}