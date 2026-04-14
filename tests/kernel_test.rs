//! AIKernel unit tests
//!
//! Tests cover: kernel creation, object CRUD, semantic operations,
//! agent registration, memory operations, and permission enforcement.

use plico::kernel::AIKernel;
use tempfile::tempdir;

fn make_kernel() -> (AIKernel, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let kernel = AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
    (kernel, dir)
}

#[test]
fn test_kernel_create_and_get() {
    let (kernel, _dir) = make_kernel();

    let cid = kernel
        .semantic_create(
            b"Meeting notes for Project X".to_vec(),
            vec!["meeting".to_string(), "project-x".to_string()],
            "TestAgent",
            Some("Quarterly kickoff notes".to_string()),
        )
        .expect("create failed");

    let obj = kernel
        .get_object(&cid, "TestAgent")
        .expect("get failed");

    assert_eq!(obj.data, b"Meeting notes for Project X");
    assert_eq!(obj.meta.tags, vec!["meeting", "project-x"]);
    assert_eq!(obj.meta.created_by, "TestAgent");
}

#[test]
fn test_kernel_semantic_create_and_read() {
    let (kernel, _dir) = make_kernel();

    let cid = kernel
        .semantic_create(
            b"Rust async programming discussion".to_vec(),
            vec!["rust".to_string(), "async".to_string()],
            "DevAgent",
            None,
        )
        .expect("create failed");

    // Read by CID
    let objs = kernel
        .semantic_read(&plico::fs::Query::ByCid(cid.clone()), "DevAgent")
        .expect("read failed");
    assert_eq!(objs.len(), 1);
    assert_eq!(objs[0].data, b"Rust async programming discussion");
}

#[test]
fn test_kernel_read_by_tags() {
    let (kernel, _dir) = make_kernel();

    kernel.semantic_create(b"doc1".to_vec(), vec!["a".to_string()], "x", None).ok();
    kernel.semantic_create(b"doc2".to_vec(), vec!["a".to_string(), "b".to_string()], "x", None).ok();
    kernel.semantic_create(b"doc3".to_vec(), vec!["b".to_string()], "x", None).ok();

    let objs = kernel
        .semantic_read(&plico::fs::Query::ByTags(vec!["a".to_string()]), "x")
        .expect("read by tags failed");

    assert_eq!(objs.len(), 2);
}

#[test]
fn test_kernel_update_changes_cid() {
    let (kernel, _dir) = make_kernel();

    let old_cid = kernel
        .semantic_create(b"original".to_vec(), vec!["t".to_string()], "x", None)
        .expect("create failed");

    let new_cid = kernel
        .semantic_update(&old_cid, b"updated".to_vec(), None, "x")
        .expect("update failed");

    // Content changed → new CID
    assert_ne!(new_cid, old_cid);

    // Old object still exists (immutable CAS)
    let old_obj = kernel.get_object(&old_cid, "x").expect("old should exist");
    assert_eq!(old_obj.data, b"original");

    // New object has new content
    let new_obj = kernel.get_object(&new_cid, "x").expect("new should exist");
    assert_eq!(new_obj.data, b"updated");
}

#[test]
fn test_kernel_delete_requires_permission() {
    let (kernel, _dir) = make_kernel();

    let cid = kernel
        .semantic_create(b"secret".to_vec(), vec!["private".to_string()], "x", None)
        .expect("create failed");

    // 'cli' agent has no Delete grant by default
    let result = kernel.semantic_delete(&cid, "cli");
    assert!(result.is_err(), "delete should fail without permission");

    // Object still readable (logical delete only)
    let obj = kernel.get_object(&cid, "cli").expect("should still exist");
    assert_eq!(obj.data, b"secret");
}

#[test]
fn test_kernel_agent_registration() {
    let (kernel, _dir) = make_kernel();

    let id = kernel.register_agent("MyAgent".to_string());
    assert!(!id.is_empty());

    let agents = kernel.list_agents();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].name, "MyAgent");
}

#[test]
fn test_kernel_remember_and_recall() {
    let (kernel, _dir) = make_kernel();

    kernel.remember("agent1", "Remember to check the logs".to_string());
    let memories = kernel.recall("agent1");

    assert!(!memories.is_empty());
    assert!(memories.iter().any(|m| m.content.display().contains("logs")));
}

#[test]
fn test_kernel_forget_ephemeral() {
    let (kernel, _dir) = make_kernel();

    kernel.remember("agent1", "Temporary note".to_string());
    assert!(!kernel.recall("agent1").is_empty());

    kernel.forget_ephemeral("agent1");
    let memories = kernel.recall("agent1");

    // Ephemeral tier entries should be gone
    let ephemeral: Vec<_> = memories
        .iter()
        .filter(|m| matches!(m.tier, plico::memory::MemoryTier::Ephemeral))
        .collect();
    assert!(ephemeral.is_empty(), "ephemeral memories should be cleared");
}

#[test]
fn test_kernel_list_tags() {
    let (kernel, _dir) = make_kernel();

    kernel.semantic_create(b"doc1".to_vec(), vec!["a".to_string(), "b".to_string()], "x", None).ok();
    kernel.semantic_create(b"doc2".to_vec(), vec!["b".to_string(), "c".to_string()], "x", None).ok();

    let tags = kernel.list_tags();
    assert!(tags.contains(&"a".to_string()));
    assert!(tags.contains(&"b".to_string()));
    assert!(tags.contains(&"c".to_string()));
}
