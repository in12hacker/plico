# Module: scheduler — Agent Lifecycle & Intent Scheduling

Manages AI agent registration and intent scheduling via priority queue.

Status: stable | Fan-in: 2 (kernel, api) | Fan-out: 0

## Public API

| Export | File | Description |
|--------|------|-------------|
| `AgentScheduler` | `mod.rs` | Global scheduler: register/submit/dequeue/update_state/list_agents |
| `Agent` | `agent.rs` | Agent entity: id/name/state/current_intent/resources |
| `AgentId` | `agent.rs` | UUID-based agent identifier |
| `AgentState` | `agent.rs` | Enum: Created/Waiting/Running/Suspended/Completed/Failed/Terminated |
| `Intent` | `agent.rs` | Task: id/priority/description/agent_id/submitted_at |
| `IntentPriority` | `agent.rs` | Enum: Critical(4)/High(3)/Medium(2)/Low(1) |
| `IntentId` | `agent.rs` | UUID-based intent identifier |
| `SchedulerQueue` | `queue.rs` | Binary heap queue: push/pop/peek/len |
| `SchedulerError` | `queue.rs` | Error: Empty, IntentNotFound |

## Dependencies (Fan-out: 0)

Leaf module.

## Dependents (Fan-in: 2)

- `src/kernel/mod.rs` → `AgentScheduler::register`, `submit`, `list_agents`
- `src/bin/plicod.rs` → `AgentScheduler` via kernel

## Interface Contract

- `AgentScheduler::register(agent)`: Returns `AgentId`. **Side effect**: inserts into agents map.
- `AgentScheduler::submit(intent)`: Enqueues intent. **Side effect**: acquires write lock.
- `SchedulerQueue::pop()`: Returns highest-priority (Critical > High > Medium > Low), oldest (lowest timestamp) intent.
- `IntentPriority::cmp()`: Ord by discriminant value (higher = more urgent).

## Modification Risk

- Change `IntentPriority` ordering → affects scheduling fairness; test `test_priority_ordering`
- Add new `AgentState` → update all pattern matches; **BREAKING** if terminal states change
- Change queue from BinaryHeap to another structure → affects scheduling behavior

## Task Routing

- Add agent priority/weighting → modify `Agent` + scheduling logic
- Change scheduling algorithm → modify `SchedulerQueue` or add new scheduler type
- Add agent resource limits → modify `AgentResources`, enforce in `AIKernel`
- Add agent communication (IPC) → new module `scheduler/ipc.rs`, update `AgentScheduler`

## Tests

- `cargo test --lib -- scheduler::tests::test_priority_ordering` — queue ordering
- `cargo test --lib -- scheduler::tests::test_register_and_list` — agent registration
