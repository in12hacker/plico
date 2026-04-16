# Module: scheduler

Agent lifecycle management — creation, scheduling, priority-based dispatch, suspension, destruction, per-agent resource limits, and kernel-mediated messaging.

Status: stable | Fan-in: 2 | Fan-out: 0

## Dependents (Fan-in: 2)

- `src/kernel/mod.rs` → AgentScheduler, Agent, Intent, AgentResources, MessageBus types
- `src/bin/plicod.rs` [indirect via kernel] → agent operations through API

## Modification Risk

- Add `AgentState` variant → BREAKING, update all match arms
- Change `IntentPriority` ordering → behavioral change, affects scheduling order
- Change `AgentExecutor` trait → BREAKING, update all implementations
- Change `AgentMessage` / `MessageBus` API → update kernel + API + CLI

## Task Routing

- Add agent state → `src/scheduler/agent.rs` `AgentState` enum
- Change priority ordering → `src/scheduler/queue.rs` `Ord` implementation
- Add dispatch strategy → `src/scheduler/dispatch.rs`
- Change agent lifecycle or resources → `src/scheduler/agent.rs`
- Mailbox behavior / capacity → `src/scheduler/messaging.rs`

## Public API

| Export | File | Description |
|--------|------|-------------|
| `Agent` | `agent.rs` | Agent with ID, name, state, capabilities, resources |
| `AgentResources` | `agent.rs` | memory_quota (entry count), cpu_time_quota (ms), allowed_tools |
| `AgentId` | `agent.rs` | String-based agent identifier |
| `AgentState` | `agent.rs` | Lifecycle state |
| `Intent` | `agent.rs` | Task with priority and description |
| `IntentPriority` | `agent.rs` | Critical / High / Medium / Low |
| `AgentScheduler` | `mod.rs` | Registry + queue + `get_resources` / `set_resources` |
| `AgentMessage` | `messaging.rs` | Inter-agent message record |
| `MessageBus` | `messaging.rs` | Bounded mailboxes: send, read, ack |
| `SchedulerQueue` | `queue.rs` | Binary heap priority queue |
| `AgentExecutor` | `dispatch.rs` | Intent execution backends |
| `TokioDispatchLoop` | `dispatch.rs` | Async dispatch loop |
| `DispatchHandle` | `dispatch.rs` | Handle to running dispatch loop |
| `LocalExecutor` | `dispatch.rs` | Synchronous executor |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `agent.rs` | ~221 | Agent, AgentId, AgentState, Intent, AgentResources |
| `queue.rs` | ~90 | SchedulerQueue |
| `dispatch.rs` | ~412 | AgentExecutor, TokioDispatchLoop, KernelExecutor |
| `messaging.rs` | ~183 | MessageBus, AgentMessage |
| `mod.rs` | ~213 | AgentScheduler, AgentHandle |

## Dependencies (Fan-out: 0)

Std + `uuid`, `serde`, `tokio`, `serde_json` (messaging payload).

## Interface Contract

- `AgentScheduler::register()`: stores agent, returns `AgentId`
- `AgentScheduler::submit()`: enqueues intent by priority
- `AgentScheduler::dequeue()`: highest priority first; FIFO within same priority
- `MessageBus::send`: returns message id; may evict oldest when mailbox full
- Thread safety: `RwLock` on registry and bus internals

## Tests

- Unit: `src/scheduler/mod.rs`, `src/scheduler/dispatch.rs`
- Integration: `tests/kernel_test.rs` (agents, messaging, resources)
