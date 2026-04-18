//! CLI command handlers — split into focused submodules.
//!
//! Architecture: directory module with one file per command group.
//! Each submodule's public items are re-exported here for backward compatibility.

pub mod crud;
pub mod agent;
pub mod memory;
pub mod graph;
pub mod deleted;
pub mod intent;
pub mod messaging;
pub mod tool;
pub mod events;
pub mod context;

// Re-export shared utilities for handler submodules.
// Re-export shared utilities for handler submodules (defined in parent commands/mod.rs).
#[allow(unused_imports)]
pub use super::{extract_arg, extract_tags, extract_tags_opt};

// Re-export all public command functions for use by the parent module.
pub use crud::{cmd_create, cmd_read, cmd_search, cmd_update, cmd_delete, cmd_history, cmd_rollback};
pub use agent::{
    cmd_agent, cmd_agents, cmd_agent_status,
    cmd_agent_suspend, cmd_agent_resume, cmd_agent_terminate,
    cmd_agent_complete, cmd_agent_fail,
};
pub use memory::{
    cmd_remember, cmd_recall, cmd_tags,
    cmd_memmove, cmd_memdelete,
};
pub use graph::{
    cmd_explore, cmd_add_node, cmd_add_edge,
    cmd_list_nodes, cmd_find_paths,
    cmd_get_node, cmd_list_edges, cmd_rm_node, cmd_rm_edge, cmd_update_node,
    cmd_edge_history,
};
pub use deleted::{cmd_deleted, cmd_restore};
pub use intent::cmd_intent;
pub use messaging::{cmd_send_message, cmd_read_messages, cmd_ack_message};
pub use tool::cmd_tool;
pub use events::cmd_events;
pub use context::cmd_context;
