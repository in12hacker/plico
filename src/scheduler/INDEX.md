# Module: scheduler

Agent lifecycle management — creation, scheduling, priority-based dispatch, suspension, and destruction.

Status: stable | Fan-in: 2 | Fan-out: 0

## Dependents (Fan-in: 2)

- `src/kernel/mod.rs` → AgentScheduler, Agent, Intent, IntentPriority, AgentHandle, AgentId, DispatchHandle, TokioDispatchLoop, LocalExecutor, AgentExecutor
- `src/bin/plicod.rs` [indirect via kernel] → agent operations through API

## Modification Risk

- Add `AgentState` variant → BREAKING, update all match arms
- Change `IntentPriority` ordering → behavioral change, affects scheduling order
- Change `AgentExecutor` trait → BREAKING, update all implementations
- Add field to `Agent` → compatible, update constructors

## Task Routing

- Add agent state → modify `src/scheduler/agent.rs` AgentState enum
- Change priority ordering → modify `src/scheduler/queue.rs` Ord implementation
- Add dispatch strategy → modify `src/scheduler/dispatch.rs`
- Change agent lifecycle → modify `src/scheduler/agent.rs` Agent + AgentState

## Public API

| Export | File | Description |
|--------|------|-------------|
| `Agent` | `agent.rs` | AI agent with ID, name, state, capabilities |
| `AgentId` | `agent.rs` | UUID-based agent identifier |
| `AgentState` | `agent.rs` | Lifecycle state (Created/Waiting/Running/etc.) |
| `Intent` | `agent.rs` | Task/goal with priority and description |
| `IntentPriority` | `agent.rs` | Priority levels (Critical/High/Medium/Low) |
| `AgentScheduler` | `mod.rs` | Global agent registry + intent queue |
| `SchedulerQueue` | `queue.rs` | Binary heap priority queue |
| `AgentExecutor` | `dispatch.rs` | Trait for intent execution backends |
| `TokioDispatchLoop` | `dispatch.rs` | Async dispatch loop for tokio runtime |
| `DispatchHandle` | `dispatch.rs` | Handle to running dispatch loop |
| `LocalExecutor` | `dispatch.rs` | Simple synchronous executor |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `agent.rs` | ~196 | Agent, AgentId, AgentState, Intent, IntentPriority |
| `queue.rs` | ~90 | SchedulerQueue (BinaryHeap-based) |
| `dispatch.rs` | ~412 | AgentExecutor trait, TokioDispatchLoop, DispatchHandle |
| `mod.rs` | ~154 | AgentScheduler (registry + queue), AgentHandle |

## Dependencies (Fan-out: 0)

None — scheduler is a standalone module, depends only on std + external crates (uuid, serde, tokio).

## Interface Contract

- `AgentScheduler::register()`: stores agent, returns AgentId
- `AgentScheduler::submit()`: adds intent to priority queue
- `AgentScheduler::dequeue()`: returns highest-priority intent (Critical > High > Medium > Low; FIFO within same priority)
- `TokioDispatchLoop::start()`: spawns background task, returns DispatchHandle
- Thread safety: all methods use `RwLock` — safe for concurrent access

## Tests

- Unit: `src/scheduler/mod.rs` mod tests, `src/scheduler/dispatch.rs` mod tests
- Integration: `tests/kernel_test.rs` (agent register/submit through kernel)
- Critical: `test_priority_ordering`, `test_register_and_list`
