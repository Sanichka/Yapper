//! Bounded two-lane TTS queue.
//!
//! Gifts are high priority; chat is normal. When the queue is full the oldest
//! normal chat message is evicted so speech never lags behind the live stream.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::MAX_QUEUE_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    High,
    Normal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub id: u64,
    /// Display name (nickname, or handle when TikTok omits the nickname).
    pub user: String,
    pub unique_id: String,
    /// Text to synthesize (template already applied).
    pub text: String,
    /// Original chat comment / gift description.
    pub raw: String,
    pub priority: Priority,
    /// `chat` or `gift`.
    pub kind: String,
    pub voice: String,
    pub speed: f32,
    /// Unix ms when the item was enqueued.
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueSnapshot {
    pub active: Option<QueueItem>,
    pub items: Vec<QueueItem>,
    pub dropped: u64,
    pub capacity: usize,
}

#[derive(Debug)]
pub enum PushOutcome {
    Enqueued {
        id: u64,
    },
    /// Enqueued after evicting this old normal-priority item.
    Evicted(QueueItem),
    /// Queue is full of high-priority items; the incoming item was dropped.
    Rejected(QueueItem),
}

pub struct BoundedQueue {
    items: VecDeque<QueueItem>,
    active: Option<QueueItem>,
    capacity: usize,
    dropped: u64,
    next_id: u64,
}

impl Default for BoundedQueue {
    fn default() -> Self {
        Self::new(MAX_QUEUE_SIZE)
    }
}

impl BoundedQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            items: VecDeque::with_capacity(capacity.max(1)),
            active: None,
            capacity: capacity.max(1),
            dropped: 0,
            next_id: 1,
        }
    }

    pub fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn active(&self) -> Option<&QueueItem> {
        self.active.as_ref()
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        while self.items.len() > self.capacity {
            if self.evict_oldest_normal().is_none() {
                self.items.pop_back();
            }
        }
    }

    pub fn enqueue(&mut self, item: QueueItem) -> PushOutcome {
        if self.items.len() < self.capacity {
            let id = item.id;
            self.insert(item);
            return PushOutcome::Enqueued { id };
        }

        match item.priority {
            Priority::High => match self.evict_oldest_normal() {
                Some(evicted) => {
                    self.insert(item);
                    PushOutcome::Evicted(evicted)
                }
                None => {
                    self.dropped += 1;
                    PushOutcome::Rejected(item)
                }
            },
            Priority::Normal => {
                if self.items.iter().any(|i| i.priority == Priority::Normal) {
                    let evicted = self.evict_oldest_normal().expect("checked above");
                    self.insert(item);
                    PushOutcome::Evicted(evicted)
                } else {
                    self.dropped += 1;
                    PushOutcome::Rejected(item)
                }
            }
        }
    }

    /// Inserts after the last high-priority item, preserving FIFO within a lane.
    fn insert(&mut self, item: QueueItem) {
        if item.priority == Priority::High {
            let at = self
                .items
                .iter()
                .rposition(|i| i.priority == Priority::High)
                .map(|i| i + 1)
                .unwrap_or(0);
            self.items.insert(at, item);
        } else {
            self.items.push_back(item);
        }
    }

    fn evict_oldest_normal(&mut self) -> Option<QueueItem> {
        let index = self
            .items
            .iter()
            .position(|i| i.priority == Priority::Normal)?;
        self.items.remove(index)
    }

    /// Pops the next item and marks it as the active one.
    pub fn take_next(&mut self) -> Option<QueueItem> {
        if self.active.is_some() {
            return None;
        }
        self.active = self.items.pop_front();
        self.active.clone()
    }

    /// Marks the active item done and returns it.
    pub fn finish_active(&mut self) -> Option<QueueItem> {
        self.active.take()
    }

    /// Removes a pending item by id.
    pub fn remove(&mut self, id: u64) -> Option<QueueItem> {
        let index = self.items.iter().position(|i| i.id == id)?;
        self.items.remove(index)
    }

    /// Drops every waiting item (the active item is untouched). Returns the count.
    pub fn clear_pending(&mut self) -> usize {
        let count = self.items.len();
        self.items.clear();
        count
    }

    /// Drops the active item, e.g. when the user presses "skip".
    pub fn skip_active(&mut self) -> Option<QueueItem> {
        self.active.take()
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            active: self.active.clone(),
            items: self.items.iter().cloned().collect(),
            dropped: self.dropped,
            capacity: self.capacity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u64, priority: Priority) -> QueueItem {
        QueueItem {
            id,
            user: "u".into(),
            unique_id: "u".into(),
            text: "t".into(),
            raw: "t".into(),
            priority,
            kind: "chat".into(),
            voice: "Bella".into(),
            speed: 1.0,
            created_at: 0,
        }
    }

    #[test]
    fn high_priority_jumps_the_line() {
        let mut q = BoundedQueue::new(4);
        q.enqueue(item(1, Priority::Normal));
        q.enqueue(item(2, Priority::Normal));
        q.enqueue(item(3, Priority::High));
        q.enqueue(item(4, Priority::High));
        let ids: Vec<u64> = q.items.iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![3, 4, 1, 2]);
    }

    #[test]
    fn full_queue_evicts_oldest_normal() {
        let mut q = BoundedQueue::new(2);
        q.enqueue(item(1, Priority::Normal));
        q.enqueue(item(2, Priority::High));
        match q.enqueue(item(3, Priority::High)) {
            PushOutcome::Evicted(evicted) => assert_eq!(evicted.id, 1),
            other => panic!("expected eviction, got {other:?}"),
        }
        assert_eq!(q.len(), 2);
        assert!(q.items.iter().all(|i| i.priority == Priority::High));
    }

    #[test]
    fn normal_is_rejected_when_full_of_gifts() {
        let mut q = BoundedQueue::new(1);
        q.enqueue(item(1, Priority::High));
        match q.enqueue(item(2, Priority::Normal)) {
            PushOutcome::Rejected(rejected) => assert_eq!(rejected.id, 2),
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn active_and_skip_lifecycle() {
        let mut q = BoundedQueue::new(3);
        q.enqueue(item(1, Priority::Normal));
        q.enqueue(item(2, Priority::Normal));
        assert_eq!(q.take_next().unwrap().id, 1);
        assert!(q.take_next().is_none());
        assert_eq!(q.skip_active().unwrap().id, 1);
        assert_eq!(q.take_next().unwrap().id, 2);
        assert_eq!(q.clear_pending(), 0);
        assert_eq!(q.finish_active().unwrap().id, 2);
    }
}
