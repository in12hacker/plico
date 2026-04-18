//! Kernel Event Bus — typed pub/sub for runtime event notification.
//!
//! Agents subscribe to the bus and poll for events. The kernel emits
//! events at key operation points. This is pure mechanism — the kernel
//! never decides what to do with events (that's upper-layer policy).

use std::collections::HashMap;
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
}

const DEFAULT_CAPACITY: usize = 256;

pub struct EventBus {
    sender: broadcast::Sender<KernelEvent>,
    subscriptions: RwLock<HashMap<String, Mutex<broadcast::Receiver<KernelEvent>>>>,
    next_sub_id: AtomicU64,
}

impl EventBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(DEFAULT_CAPACITY);
        Self {
            sender,
            subscriptions: RwLock::new(HashMap::new()),
            next_sub_id: AtomicU64::new(1),
        }
    }

    pub fn emit(&self, event: KernelEvent) {
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> String {
        let id = format!("sub-{}", self.next_sub_id.fetch_add(1, Ordering::Relaxed));
        let rx = self.sender.subscribe();
        self.subscriptions
            .write()
            .unwrap()
            .insert(id.clone(), Mutex::new(rx));
        id
    }

    pub fn poll(&self, subscription_id: &str) -> Option<Vec<KernelEvent>> {
        let subs = self.subscriptions.read().unwrap();
        let rx_mutex = subs.get(subscription_id)?;
        let mut rx = rx_mutex.lock().unwrap();
        let mut events = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(event) => events.push(event),
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::warn!(
                        "Subscription {} lagged by {} events",
                        subscription_id,
                        n
                    );
                    continue;
                }
                Err(broadcast::error::TryRecvError::Closed) => break,
            }
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
}
