//! Kernel Event Bus — typed pub/sub for runtime event notification.
//!
//! Agents subscribe to the bus and poll for events. The kernel emits
//! events at key operation points. This is pure mechanism — the kernel
//! never decides what to do with events (that's upper-layer policy).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum KernelEvent {
    AgentStateChanged {
        agent_id: String,
        old_state: String,
        new_state: String,
    },
    ObjectStored {
        cid: String,
        agent_id: String,
        tags: Vec<String>,
    },
    MemoryStored {
        agent_id: String,
        tier: String,
    },
    IntentSubmitted {
        intent_id: String,
        agent_id: Option<String>,
        priority: String,
    },
    IntentCompleted {
        intent_id: String,
        success: bool,
    },
    EventCreated {
        event_id: String,
        label: String,
        agent_id: String,
    },
    KnowledgeShared {
        cid: String,
        agent_id: String,
        scope: String, // "shared" | "group:{group_id}"
        tags: Vec<String>,
        summary: String, // metadata concatenation, no LLM
    },
    KnowledgeSuperseded {
        old_cid: String,
        new_cid: String,
        agent_id: String,
    },
    TaskDelegated {
        task_id: String,
        from_agent: String,
        to_agent: String,
    },
    TaskCompleted {
        task_id: String,
        agent_id: String,
        result_cids: Vec<String>,
    },
}

/// A durable event record with sequence number and timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequencedEvent {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub event: KernelEvent,
}

impl KernelEvent {
    pub fn event_type_name(&self) -> &'static str {
        match self {
            KernelEvent::AgentStateChanged { .. } => "AgentStateChanged",
            KernelEvent::ObjectStored { .. } => "ObjectStored",
            KernelEvent::MemoryStored { .. } => "MemoryStored",
            KernelEvent::IntentSubmitted { .. } => "IntentSubmitted",
            KernelEvent::IntentCompleted { .. } => "IntentCompleted",
            KernelEvent::EventCreated { .. } => "EventCreated",
            KernelEvent::KnowledgeShared { .. } => "KnowledgeShared",
            KernelEvent::KnowledgeSuperseded { .. } => "KnowledgeSuperseded",
            KernelEvent::TaskDelegated { .. } => "TaskDelegated",
            KernelEvent::TaskCompleted { .. } => "TaskCompleted",
        }
    }

    pub fn agent_id(&self) -> Option<&str> {
        match self {
            KernelEvent::AgentStateChanged { agent_id, .. } => Some(agent_id),
            KernelEvent::ObjectStored { agent_id, .. } => Some(agent_id),
            KernelEvent::MemoryStored { agent_id, .. } => Some(agent_id),
            KernelEvent::IntentSubmitted { agent_id, .. } => agent_id.as_deref(),
            KernelEvent::IntentCompleted { .. } => None,
            KernelEvent::EventCreated { agent_id, .. } => Some(agent_id),
            KernelEvent::KnowledgeShared { agent_id, .. } => Some(agent_id),
            KernelEvent::KnowledgeSuperseded { agent_id, .. } => Some(agent_id),
            KernelEvent::TaskDelegated { from_agent, .. } => Some(from_agent),
            KernelEvent::TaskCompleted { agent_id, .. } => Some(agent_id),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventFilter {
    pub event_types: Option<Vec<String>>,
    pub agent_ids: Option<Vec<String>>,
}

impl EventFilter {
    pub fn matches(&self, event: &KernelEvent) -> bool {
        if let Some(ref types) = self.event_types {
            if !types.iter().any(|t| t == event.event_type_name()) {
                return false;
            }
        }
        if let Some(ref agents) = self.agent_ids {
            match event.agent_id() {
                Some(aid) => {
                    if !agents.iter().any(|a| a == aid) {
                        return false;
                    }
                }
                None => return false,
            }
        }
        true
    }
}

const DEFAULT_BROADCAST_CAPACITY: usize = 4096;
const DEFAULT_EVENT_LOG_CAPACITY: usize = 65536;

struct Subscription {
    receiver: Mutex<broadcast::Receiver<KernelEvent>>,
    filter: Option<EventFilter>,
    last_seen_seq: AtomicU64,
}

struct RingEventLog {
    events: VecDeque<SequencedEvent>,
    max_capacity: usize,
    min_seq: u64,
}

impl RingEventLog {
    fn new(capacity: usize) -> Self {
        Self {
            events: VecDeque::with_capacity(capacity.min(1024)),
            max_capacity: capacity,
            min_seq: 0,
        }
    }

    fn push(&mut self, event: SequencedEvent) {
        if self.events.len() >= self.max_capacity {
            if let Some(evicted) = self.events.pop_front() {
                self.min_seq = evicted.seq + 1;
            }
        }
        self.events.push_back(event);
    }

    fn events_since(&self, since_seq: u64) -> Vec<SequencedEvent> {
        if since_seq < self.min_seq && since_seq > 0 {
            tracing::warn!(
                requested_seq = since_seq,
                oldest_available = self.min_seq,
                "Event log gap: requested seq evicted, returning available events"
            );
        }
        let effective_seq = since_seq.max(self.min_seq.saturating_sub(1));
        self.events
            .iter()
            .filter(|e| e.seq > effective_seq)
            .cloned()
            .collect()
    }

    fn events_by_agent(&self, agent_id: &str) -> Vec<SequencedEvent> {
        self.events
            .iter()
            .filter(|e| e.event.agent_id() == Some(agent_id))
            .cloned()
            .collect()
    }

    fn len(&self) -> usize {
        self.events.len()
    }

    fn snapshot(&self) -> Vec<SequencedEvent> {
        self.events.iter().cloned().collect()
    }

    fn restore(&mut self, events: Vec<SequencedEvent>) {
        let max_seq = events.iter().map(|e| e.seq).max().unwrap_or(0);
        if events.len() > self.max_capacity {
            let skip = events.len() - self.max_capacity;
            self.min_seq = events[skip - 1].seq + 1;
            self.events = events.into_iter().skip(skip).collect();
        } else {
            self.min_seq = 0;
            self.events = events.into();
        }
        let _ = max_seq; // next_seq is managed by EventBus
    }
}

pub struct EventBus {
    sender: broadcast::Sender<KernelEvent>,
    subscriptions: RwLock<HashMap<String, Subscription>>,
    next_sub_id: AtomicU64,
    event_log: RwLock<RingEventLog>,
    next_seq: AtomicU64,
    /// Optional path for JSONL event log persistence. When set, each emit
    /// appends to this file for crash-safe durability.
    event_log_path: Option<PathBuf>,
    /// F-25: Segment rotation — start timestamp of current segment (ms).
    segment_start_ms: AtomicU64,
    /// F-25: Rotation interval in ms (default 7 days).
    rotation_interval_ms: u64,
    /// F-25: Max archive segments to retain (default 4 = ~1 month).
    retention_segments: usize,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

const DEFAULT_ROTATION_INTERVAL_MS: u64 = 7 * 24 * 3600 * 1000; // 7 days
const DEFAULT_RETENTION_SEGMENTS: usize = 4;

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(DEFAULT_BROADCAST_CAPACITY);
        Self {
            sender,
            subscriptions: RwLock::new(HashMap::new()),
            next_sub_id: AtomicU64::new(1),
            event_log: RwLock::new(RingEventLog::new(DEFAULT_EVENT_LOG_CAPACITY)),
            next_seq: AtomicU64::new(1),
            event_log_path: None,
            segment_start_ms: AtomicU64::new(now_ms()),
            rotation_interval_ms: DEFAULT_ROTATION_INTERVAL_MS,
            retention_segments: DEFAULT_RETENTION_SEGMENTS,
        }
    }

    /// Create an EventBus that persists events to the given JSONL path on each emit.
    pub fn with_persistence(path: PathBuf) -> Self {
        let (sender, _) = broadcast::channel(DEFAULT_BROADCAST_CAPACITY);
        Self {
            sender,
            subscriptions: RwLock::new(HashMap::new()),
            next_sub_id: AtomicU64::new(1),
            event_log: RwLock::new(RingEventLog::new(DEFAULT_EVENT_LOG_CAPACITY)),
            next_seq: AtomicU64::new(1),
            event_log_path: Some(path),
            segment_start_ms: AtomicU64::new(now_ms()),
            rotation_interval_ms: DEFAULT_ROTATION_INTERVAL_MS,
            retention_segments: DEFAULT_RETENTION_SEGMENTS,
        }
    }

    /// Create an EventBus with custom rotation parameters (F-25).
    pub fn with_rotation(path: PathBuf, rotation_interval_ms: u64, retention_segments: usize) -> Self {
        let (sender, _) = broadcast::channel(DEFAULT_BROADCAST_CAPACITY);
        Self {
            sender,
            subscriptions: RwLock::new(HashMap::new()),
            next_sub_id: AtomicU64::new(1),
            event_log: RwLock::new(RingEventLog::new(DEFAULT_EVENT_LOG_CAPACITY)),
            next_seq: AtomicU64::new(1),
            event_log_path: Some(path),
            segment_start_ms: AtomicU64::new(now_ms()),
            rotation_interval_ms,
            retention_segments,
        }
    }

    /// Append a sequenced event to the JSONL log file.
    fn append_to_log(&self, entry: &SequencedEvent) -> Result<(), std::io::Error> {
        let Some(ref path) = self.event_log_path else {
            return Ok(());
        };
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let json = serde_json::to_string(entry).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e)
        })?;
        writeln!(file, "{}", json)?;
        Ok(())
    }

    // ── F-25: Segment rotation ──

    fn should_rotate(&self) -> bool {
        let start = self.segment_start_ms.load(Ordering::Relaxed);
        start > 0 && now_ms().saturating_sub(start) >= self.rotation_interval_ms
    }

    /// Rotate the current segment to an archive file and start a fresh segment.
    /// Returns Ok(Some(archive_path)) on successful rotation, Ok(None) if skipped.
    pub fn rotate_segment(&self) -> Result<Option<PathBuf>, std::io::Error> {
        let Some(ref current_path) = self.event_log_path else {
            return Ok(None);
        };
        if !current_path.exists() {
            return Ok(None);
        }

        let archive_dir = current_path.parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("event_archive");
        std::fs::create_dir_all(&archive_dir)?;

        let seg_start = self.segment_start_ms.load(Ordering::Relaxed);
        let archive_name = format!("events_{}.jsonl", seg_start);
        let archive_path = archive_dir.join(&archive_name);

        std::fs::rename(current_path, &archive_path)?;

        // Reset segment start
        self.segment_start_ms.store(now_ms(), Ordering::Relaxed);

        self.cleanup_old_archives(&archive_dir)?;
        Ok(Some(archive_path))
    }

    fn cleanup_old_archives(&self, archive_dir: &std::path::Path) -> Result<(), std::io::Error> {
        let mut archives: Vec<_> = std::fs::read_dir(archive_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("events_"))
            .collect();
        archives.sort_by_key(|e| e.file_name());

        if archives.len() > self.retention_segments {
            let to_remove = archives.len() - self.retention_segments;
            for entry in &archives[..to_remove] {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Ok(())
    }

    /// List archive segment paths in chronological order.
    pub fn list_archive_segments(&self) -> Vec<PathBuf> {
        let Some(ref current_path) = self.event_log_path else { return vec![]; };
        let archive_dir = current_path.parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("event_archive");
        if !archive_dir.exists() { return vec![]; }

        let mut segments: Vec<PathBuf> = std::fs::read_dir(&archive_dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("events_"))
            .map(|e| e.path())
            .collect();
        segments.sort();
        segments
    }

    pub fn emit(&self, event: KernelEvent) {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let sequenced = SequencedEvent {
            seq,
            timestamp_ms: now_ms(),
            event: event.clone(),
        };
        self.event_log.write().unwrap().push(sequenced.clone());
        let _ = self.sender.send(event);

        // F-25: rotate before appending if interval exceeded
        if self.should_rotate() {
            if let Err(e) = self.rotate_segment() {
                tracing::warn!("Failed to rotate event log segment: {}", e);
            }
        }

        if let Err(e) = self.append_to_log(&sequenced) {
            tracing::warn!("Failed to append event to log: {}", e);
        }
    }

    pub fn events_since(&self, since_seq: u64) -> Vec<SequencedEvent> {
        self.event_log.read().unwrap().events_since(since_seq)
    }

    pub fn events_by_agent(&self, agent_id: &str) -> Vec<SequencedEvent> {
        self.event_log.read().unwrap().events_by_agent(agent_id)
    }

    /// Number of events currently buffered (may be less than total emitted due to eviction).
    pub fn event_count(&self) -> usize {
        self.event_log.read().unwrap().len()
    }

    /// The sequence number that will be assigned to the next emitted event.
    /// Use this instead of `event_count()` when you need a monotonic position marker.
    pub fn current_seq(&self) -> u64 {
        self.next_seq.load(Ordering::Relaxed)
    }

    /// The oldest sequence number still available in the event log.
    /// Returns 0 if no events have been evicted.
    pub fn oldest_seq(&self) -> u64 {
        self.event_log.read().unwrap().min_seq
    }

    pub fn snapshot_events(&self) -> Vec<SequencedEvent> {
        self.event_log.read().unwrap().snapshot()
    }

    pub fn restore_events(&self, events: Vec<SequencedEvent>) {
        let max_seq = events.iter().map(|e| e.seq).max().unwrap_or(0);
        self.event_log.write().unwrap().restore(events);
        self.next_seq.store(max_seq + 1, Ordering::Relaxed);
    }

    /// Load events from a JSONL file on disk.
    pub fn load_event_log(path: &std::path::Path) -> Result<Vec<SequencedEvent>, std::io::Error> {
        use std::io::{BufRead, BufReader};
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = std::fs::OpenOptions::new().read(true).open(path)?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<SequencedEvent>(&line) {
                Ok(event) => events.push(event),
                Err(e) => tracing::warn!("Skipping malformed event line: {}", e),
            }
        }
        Ok(events)
    }

    pub fn subscribe(&self) -> String {
        self.subscribe_filtered(None)
    }

    pub fn subscribe_filtered(&self, filter: Option<EventFilter>) -> String {
        let id = format!("sub-{}", self.next_sub_id.fetch_add(1, Ordering::Relaxed));
        let rx = self.sender.subscribe();
        let current = self.next_seq.load(Ordering::Relaxed);
        self.subscriptions
            .write()
            .unwrap()
            .insert(id.clone(), Subscription {
                receiver: Mutex::new(rx),
                filter,
                last_seen_seq: AtomicU64::new(current),
            });
        id
    }

    pub fn poll(&self, subscription_id: &str) -> Option<Vec<KernelEvent>> {
        let subs = self.subscriptions.read().unwrap();
        let sub = subs.get(subscription_id)?;
        let mut rx = sub.receiver.lock().unwrap();
        let mut events = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(event) => {
                    if sub.filter.as_ref().is_none_or(|f| f.matches(&event)) {
                        events.push(event);
                    }
                }
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::warn!(
                        subscription = subscription_id,
                        lagged = n,
                        "Broadcast lagged, recovering from event log"
                    );
                    let last_seq = sub.last_seen_seq.load(Ordering::Relaxed);
                    let recovered = self.event_log.read().unwrap().events_since(last_seq);
                    for se in recovered {
                        if sub.filter.as_ref().is_none_or(|f| f.matches(&se.event)) {
                            events.push(se.event);
                        }
                    }
                    break;
                }
                Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }
        if !events.is_empty() {
            let current = self.next_seq.load(Ordering::Relaxed);
            sub.last_seen_seq.store(current, Ordering::Relaxed);
        }
        Some(events)
    }

    pub fn unsubscribe(&self, subscription_id: &str) -> bool {
        self.subscriptions
            .write()
            .unwrap()
            .remove(subscription_id)
            .is_some()
    }

    pub fn subscription_count(&self) -> usize {
        self.subscriptions.read().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emit_and_poll() {
        let bus = EventBus::new();
        let sub = bus.subscribe();

        bus.emit(KernelEvent::ObjectStored {
            cid: "abc123".into(),
            agent_id: "agent-1".into(),
            tags: vec!["tag-a".into()],
        });

        let events = bus.poll(&sub).unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            KernelEvent::ObjectStored { cid, .. } => assert_eq!(cid, "abc123"),
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn test_poll_empty() {
        let bus = EventBus::new();
        let sub = bus.subscribe();
        let events = bus.poll(&sub).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_poll_unknown_subscription() {
        let bus = EventBus::new();
        assert!(bus.poll("nonexistent").is_none());
    }

    #[test]
    fn test_multiple_subscribers() {
        let bus = EventBus::new();
        let sub1 = bus.subscribe();
        let sub2 = bus.subscribe();

        bus.emit(KernelEvent::AgentStateChanged {
            agent_id: "a1".into(),
            old_state: "Created".into(),
            new_state: "Waiting".into(),
        });

        let ev1 = bus.poll(&sub1).unwrap();
        let ev2 = bus.poll(&sub2).unwrap();
        assert_eq!(ev1.len(), 1);
        assert_eq!(ev2.len(), 1);
    }

    #[test]
    fn test_subscribe_after_emit_misses_prior() {
        let bus = EventBus::new();
        bus.emit(KernelEvent::EventCreated {
            event_id: "evt-1".into(),
            label: "test".into(),
            agent_id: "a1".into(),
        });

        let sub = bus.subscribe();
        let events = bus.poll(&sub).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_unsubscribe() {
        let bus = EventBus::new();
        let sub = bus.subscribe();
        assert_eq!(bus.subscription_count(), 1);
        assert!(bus.unsubscribe(&sub));
        assert_eq!(bus.subscription_count(), 0);
        assert!(!bus.unsubscribe(&sub));
    }

    #[test]
    fn test_multiple_events_ordering() {
        let bus = EventBus::new();
        let sub = bus.subscribe();

        bus.emit(KernelEvent::IntentSubmitted {
            intent_id: "i1".into(),
            agent_id: Some("a1".into()),
            priority: "High".into(),
        });
        bus.emit(KernelEvent::IntentCompleted {
            intent_id: "i1".into(),
            success: true,
        });
        bus.emit(KernelEvent::MemoryStored {
            agent_id: "a1".into(),
            tier: "working".into(),
        });

        let events = bus.poll(&sub).unwrap();
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0], KernelEvent::IntentSubmitted { .. }));
        assert!(matches!(&events[1], KernelEvent::IntentCompleted { .. }));
        assert!(matches!(&events[2], KernelEvent::MemoryStored { .. }));
    }

    #[test]
    fn test_poll_drains_events() {
        let bus = EventBus::new();
        let sub = bus.subscribe();

        bus.emit(KernelEvent::ObjectStored {
            cid: "c1".into(),
            agent_id: "a1".into(),
            tags: vec![],
        });

        let first = bus.poll(&sub).unwrap();
        assert_eq!(first.len(), 1);

        let second = bus.poll(&sub).unwrap();
        assert!(second.is_empty());
    }

    #[test]
    fn test_kernel_event_clone_and_eq() {
        let event = KernelEvent::AgentStateChanged {
            agent_id: "a1".into(),
            old_state: "Running".into(),
            new_state: "Suspended".into(),
        };
        let cloned = event.clone();
        assert_eq!(event, cloned);
    }

    #[test]
    fn test_filter_by_event_type() {
        let bus = EventBus::new();
        let filter = EventFilter {
            event_types: Some(vec!["ObjectStored".into()]),
            agent_ids: None,
        };
        let sub = bus.subscribe_filtered(Some(filter));

        bus.emit(KernelEvent::AgentStateChanged {
            agent_id: "a1".into(),
            old_state: "Created".into(),
            new_state: "Waiting".into(),
        });
        bus.emit(KernelEvent::ObjectStored {
            cid: "c1".into(),
            agent_id: "a1".into(),
            tags: vec!["t1".into()],
        });
        bus.emit(KernelEvent::MemoryStored {
            agent_id: "a1".into(),
            tier: "working".into(),
        });

        let events = bus.poll(&sub).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], KernelEvent::ObjectStored { cid, .. } if cid == "c1"));
    }

    #[test]
    fn test_filter_by_agent_id() {
        let bus = EventBus::new();
        let filter = EventFilter {
            event_types: None,
            agent_ids: Some(vec!["agent-b".into()]),
        };
        let sub = bus.subscribe_filtered(Some(filter));

        bus.emit(KernelEvent::ObjectStored {
            cid: "c1".into(),
            agent_id: "agent-a".into(),
            tags: vec![],
        });
        bus.emit(KernelEvent::ObjectStored {
            cid: "c2".into(),
            agent_id: "agent-b".into(),
            tags: vec![],
        });
        bus.emit(KernelEvent::MemoryStored {
            agent_id: "agent-a".into(),
            tier: "long_term".into(),
        });

        let events = bus.poll(&sub).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], KernelEvent::ObjectStored { cid, .. } if cid == "c2"));
    }

    #[test]
    fn test_filter_combined_and_semantics() {
        let bus = EventBus::new();
        let filter = EventFilter {
            event_types: Some(vec!["ObjectStored".into()]),
            agent_ids: Some(vec!["agent-x".into()]),
        };
        let sub = bus.subscribe_filtered(Some(filter));

        // Matches type but not agent
        bus.emit(KernelEvent::ObjectStored {
            cid: "c1".into(),
            agent_id: "agent-y".into(),
            tags: vec![],
        });
        // Matches agent but not type
        bus.emit(KernelEvent::MemoryStored {
            agent_id: "agent-x".into(),
            tier: "working".into(),
        });
        // Matches both
        bus.emit(KernelEvent::ObjectStored {
            cid: "c2".into(),
            agent_id: "agent-x".into(),
            tags: vec![],
        });

        let events = bus.poll(&sub).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], KernelEvent::ObjectStored { cid, .. } if cid == "c2"));
    }

    #[test]
    fn test_filter_none_receives_all() {
        let bus = EventBus::new();
        let sub = bus.subscribe_filtered(None);

        bus.emit(KernelEvent::AgentStateChanged {
            agent_id: "a1".into(),
            old_state: "Created".into(),
            new_state: "Running".into(),
        });
        bus.emit(KernelEvent::ObjectStored {
            cid: "c1".into(),
            agent_id: "a1".into(),
            tags: vec![],
        });

        let events = bus.poll(&sub).unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_filter_multiple_event_types() {
        let bus = EventBus::new();
        let filter = EventFilter {
            event_types: Some(vec!["ObjectStored".into(), "MemoryStored".into()]),
            agent_ids: None,
        };
        let sub = bus.subscribe_filtered(Some(filter));

        bus.emit(KernelEvent::AgentStateChanged {
            agent_id: "a1".into(),
            old_state: "Created".into(),
            new_state: "Running".into(),
        });
        bus.emit(KernelEvent::ObjectStored {
            cid: "c1".into(),
            agent_id: "a1".into(),
            tags: vec![],
        });
        bus.emit(KernelEvent::MemoryStored {
            agent_id: "a1".into(),
            tier: "working".into(),
        });
        bus.emit(KernelEvent::IntentCompleted {
            intent_id: "i1".into(),
            success: true,
        });

        let events = bus.poll(&sub).unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], KernelEvent::ObjectStored { .. }));
        assert!(matches!(&events[1], KernelEvent::MemoryStored { .. }));
    }

    #[test]
    fn test_filter_excludes_events_without_agent_id() {
        let bus = EventBus::new();
        let filter = EventFilter {
            event_types: None,
            agent_ids: Some(vec!["a1".into()]),
        };
        let sub = bus.subscribe_filtered(Some(filter));

        // IntentCompleted has no agent_id field → filtered out
        bus.emit(KernelEvent::IntentCompleted {
            intent_id: "i1".into(),
            success: true,
        });
        bus.emit(KernelEvent::ObjectStored {
            cid: "c1".into(),
            agent_id: "a1".into(),
            tags: vec![],
        });

        let events = bus.poll(&sub).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], KernelEvent::ObjectStored { .. }));
    }

    #[test]
    fn test_event_type_name() {
        assert_eq!(KernelEvent::AgentStateChanged {
            agent_id: "a".into(), old_state: "x".into(), new_state: "y".into(),
        }.event_type_name(), "AgentStateChanged");
        assert_eq!(KernelEvent::ObjectStored {
            cid: "c".into(), agent_id: "a".into(), tags: vec![],
        }.event_type_name(), "ObjectStored");
        assert_eq!(KernelEvent::IntentCompleted {
            intent_id: "i".into(), success: true,
        }.event_type_name(), "IntentCompleted");
    }

    #[test]
    fn test_event_agent_id_extraction() {
        assert_eq!(KernelEvent::AgentStateChanged {
            agent_id: "a1".into(), old_state: "x".into(), new_state: "y".into(),
        }.agent_id(), Some("a1"));
        assert_eq!(KernelEvent::IntentSubmitted {
            intent_id: "i".into(), agent_id: None, priority: "Low".into(),
        }.agent_id(), None);
        assert_eq!(KernelEvent::IntentCompleted {
            intent_id: "i".into(), success: true,
        }.agent_id(), None);
    }

    #[test]
    fn test_event_log_records_all_emits() {
        let bus = EventBus::new();
        assert_eq!(bus.event_count(), 0);

        bus.emit(KernelEvent::ObjectStored {
            cid: "c1".into(), agent_id: "a1".into(), tags: vec![],
        });
        bus.emit(KernelEvent::MemoryStored {
            agent_id: "a1".into(), tier: "working".into(),
        });

        assert_eq!(bus.event_count(), 2);
    }

    #[test]
    fn test_event_log_sequencing() {
        let bus = EventBus::new();
        bus.emit(KernelEvent::ObjectStored {
            cid: "c1".into(), agent_id: "a1".into(), tags: vec![],
        });
        bus.emit(KernelEvent::MemoryStored {
            agent_id: "a1".into(), tier: "working".into(),
        });
        bus.emit(KernelEvent::IntentCompleted {
            intent_id: "i1".into(), success: true,
        });

        let log = bus.snapshot_events();
        assert_eq!(log.len(), 3);
        assert_eq!(log[0].seq, 1);
        assert_eq!(log[1].seq, 2);
        assert_eq!(log[2].seq, 3);
        assert!(log[0].timestamp_ms <= log[1].timestamp_ms);
        assert!(log[1].timestamp_ms <= log[2].timestamp_ms);
    }

    #[test]
    fn test_events_since() {
        let bus = EventBus::new();
        bus.emit(KernelEvent::ObjectStored {
            cid: "c1".into(), agent_id: "a1".into(), tags: vec![],
        });
        bus.emit(KernelEvent::ObjectStored {
            cid: "c2".into(), agent_id: "a1".into(), tags: vec![],
        });
        bus.emit(KernelEvent::ObjectStored {
            cid: "c3".into(), agent_id: "a1".into(), tags: vec![],
        });

        let since_0 = bus.events_since(0);
        assert_eq!(since_0.len(), 3);

        let since_1 = bus.events_since(1);
        assert_eq!(since_1.len(), 2);
        assert_eq!(since_1[0].seq, 2);

        let since_3 = bus.events_since(3);
        assert!(since_3.is_empty());
    }

    #[test]
    fn test_events_by_agent() {
        let bus = EventBus::new();
        bus.emit(KernelEvent::ObjectStored {
            cid: "c1".into(), agent_id: "agent-a".into(), tags: vec![],
        });
        bus.emit(KernelEvent::ObjectStored {
            cid: "c2".into(), agent_id: "agent-b".into(), tags: vec![],
        });
        bus.emit(KernelEvent::MemoryStored {
            agent_id: "agent-a".into(), tier: "working".into(),
        });
        bus.emit(KernelEvent::IntentCompleted {
            intent_id: "i1".into(), success: true,
        });

        let a_events = bus.events_by_agent("agent-a");
        assert_eq!(a_events.len(), 2);

        let b_events = bus.events_by_agent("agent-b");
        assert_eq!(b_events.len(), 1);

        let none_events = bus.events_by_agent("nonexistent");
        assert!(none_events.is_empty());
    }

    #[test]
    fn test_snapshot_and_restore() {
        let bus = EventBus::new();
        bus.emit(KernelEvent::ObjectStored {
            cid: "c1".into(), agent_id: "a1".into(), tags: vec![],
        });
        bus.emit(KernelEvent::MemoryStored {
            agent_id: "a1".into(), tier: "long_term".into(),
        });

        let snapshot = bus.snapshot_events();
        assert_eq!(snapshot.len(), 2);

        let bus2 = EventBus::new();
        bus2.restore_events(snapshot);
        assert_eq!(bus2.event_count(), 2);

        bus2.emit(KernelEvent::IntentCompleted {
            intent_id: "i1".into(), success: true,
        });
        assert_eq!(bus2.event_count(), 3);
        let log = bus2.snapshot_events();
        assert_eq!(log[2].seq, 3);
    }

    #[test]
    fn test_emit_records_to_both_broadcast_and_log() {
        let bus = EventBus::new();
        let sub = bus.subscribe();

        bus.emit(KernelEvent::ObjectStored {
            cid: "c1".into(), agent_id: "a1".into(), tags: vec![],
        });

        let polled = bus.poll(&sub).unwrap();
        assert_eq!(polled.len(), 1);
        assert_eq!(bus.event_count(), 1);
    }

    #[test]
    fn test_current_seq_increments() {
        let bus = EventBus::new();
        assert_eq!(bus.current_seq(), 1);

        bus.emit(KernelEvent::ObjectStored {
            cid: "c1".into(), agent_id: "a1".into(), tags: vec![],
        });
        assert_eq!(bus.current_seq(), 2);

        bus.emit(KernelEvent::MemoryStored {
            agent_id: "a1".into(), tier: "working".into(),
        });
        assert_eq!(bus.current_seq(), 3);
    }

    #[test]
    fn test_ring_buffer_eviction() {
        let bus = EventBus::new();
        {
            let mut log = bus.event_log.write().unwrap();
            *log = RingEventLog::new(5);
        }

        for i in 0..10 {
            bus.emit(KernelEvent::ObjectStored {
                cid: format!("c{}", i), agent_id: "a1".into(), tags: vec![],
            });
        }

        assert_eq!(bus.event_count(), 5);
        assert_eq!(bus.current_seq(), 11);
        assert!(bus.oldest_seq() > 0);

        let all = bus.snapshot_events();
        assert_eq!(all.len(), 5);
        assert_eq!(all[0].seq, 6);
        assert_eq!(all[4].seq, 10);
    }

    #[test]
    fn test_events_since_with_gap() {
        let bus = EventBus::new();
        {
            let mut log = bus.event_log.write().unwrap();
            *log = RingEventLog::new(5);
        }

        for i in 0..10 {
            bus.emit(KernelEvent::ObjectStored {
                cid: format!("c{}", i), agent_id: "a1".into(), tags: vec![],
            });
        }

        let from_old = bus.events_since(2);
        assert_eq!(from_old.len(), 5);
        assert_eq!(from_old[0].seq, 6);

        let from_recent = bus.events_since(8);
        assert_eq!(from_recent.len(), 2);
        assert_eq!(from_recent[0].seq, 9);
    }

    #[test]
    fn test_restore_with_capacity_overflow() {
        let bus = EventBus::new();
        {
            let mut log = bus.event_log.write().unwrap();
            *log = RingEventLog::new(3);
        }

        let events: Vec<SequencedEvent> = (1..=10).map(|i| SequencedEvent {
            seq: i,
            timestamp_ms: 1000 + i,
            event: KernelEvent::ObjectStored {
                cid: format!("c{}", i), agent_id: "a1".into(), tags: vec![],
            },
        }).collect();

        bus.restore_events(events);
        assert_eq!(bus.event_count(), 3);
        assert_eq!(bus.current_seq(), 11);

        let snap = bus.snapshot_events();
        assert_eq!(snap[0].seq, 8);
        assert_eq!(snap[2].seq, 10);
    }
}
