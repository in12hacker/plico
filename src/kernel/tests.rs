//! Test helpers for kernel inline #[cfg(test)] modules.
//!
//! Mirrors the helpers from `tests/kernel_test.rs` so that ops modules
//! can use `make_kernel()` without importing from the test crate.

#[cfg(test)]
pub fn make_kernel() -> (std::sync::Arc<crate::kernel::AIKernel>, tempfile::TempDir) {
    make_kernel_arc()
}

#[cfg(test)]
pub fn make_kernel_arc() -> (std::sync::Arc<crate::kernel::AIKernel>, tempfile::TempDir) {
    std::env::set_var("EMBEDDING_BACKEND", "stub");
    std::env::set_var("LLM_BACKEND", "stub");
    let dir = tempfile::tempdir().unwrap();
    let kernel = crate::kernel::AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
    (kernel, dir)
}

#[tokio::test]
async fn explicit_projection_shutdown_is_idempotent_and_releases_the_vault_lease() {
    let (kernel, directory) = make_kernel();
    kernel.start_workers();
    kernel.shutdown_projection_worker();
    kernel.shutdown_projection_worker();
    let vault_weak = kernel.projection.vault_weak_for_test();
    let projection_weak = std::sync::Arc::downgrade(&kernel.projection);
    let weak = std::sync::Arc::downgrade(&kernel);
    drop(kernel);
    assert!(
        weak.upgrade().is_none(),
        "background worker retained AIKernel after shutdown"
    );
    assert!(
        projection_weak.upgrade().is_none(),
        "projection runtime survived explicit shutdown"
    );
    assert!(
        vault_weak.upgrade().is_none(),
        "background component retained the vault lease after shutdown"
    );

    let reopened = crate::cas::PersonalVaultStorage::open(directory.path(), None).unwrap();
    assert!(!reopened.created_this_open());
}
