//! Runtime events are retained by the worker, independently of any webview.
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventFrame {
    pub sequence: u64,
    pub event: String,
    pub payload: Value,
}

#[derive(Default)]
struct EventState {
    sequence: u64,
    latest: BTreeMap<String, EventFrame>,
    operation_id: Option<String>,
}

#[derive(Clone, Default)]
pub struct EventBus(Arc<Mutex<EventState>>);

impl EventBus {
    pub fn clear(&self, event: &str) {
        if let Ok(mut state) = self.0.lock() {
            state.latest.remove(event);
        }
    }
    pub fn operation(&self, id: Option<String>) {
        if let Ok(mut state) = self.0.lock() {
            state.operation_id = id;
        }
    }
    pub fn emit<T: Serialize>(&self, event: &str, payload: T) -> AppResult<()> {
        let mut payload = serde_json::to_value(payload)?;
        let mut state = self
            .0
            .lock()
            .map_err(|_| AppError::new("Повреждена очередь событий."))?;
        if let Some(object) = payload.as_object_mut() {
            if event.starts_with("launcher://") {
                object.insert(
                    "operation_id".into(),
                    serde_json::to_value(&state.operation_id)?,
                );
            }
        }
        state.sequence += 1;
        let frame = EventFrame {
            sequence: state.sequence,
            event: event.into(),
            payload,
        };
        state.latest.insert(event.into(), frame);
        Ok(())
    }
    pub fn sequence(&self) -> u64 {
        self.0.lock().map(|v| v.sequence).unwrap_or(0)
    }
    pub fn since(&self, sequence: u64) -> Vec<EventFrame> {
        let Ok(state) = self.0.lock() else {
            return Vec::new();
        };
        let sequence = if sequence > state.sequence {
            0
        } else {
            sequence
        };
        let mut result: Vec<_> = state
            .latest
            .values()
            .filter(|e| e.sequence > sequence)
            .cloned()
            .collect();
        result.sort_by_key(|e| e.sequence);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reopening_observes_latest_progress_and_keeps_operation_identity() {
        let bus = EventBus::default();
        bus.operation(Some("operation-a".into()));
        bus.emit("launcher://progress", serde_json::json!({"bytes_done": 1}))
            .unwrap();
        bus.emit("launcher://progress", serde_json::json!({"bytes_done": 9}))
            .unwrap();
        bus.emit("launcher://ready", serde_json::json!({"ready": true}))
            .unwrap();
        let frames = bus.since(0);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].payload["bytes_done"], 9);
        assert_eq!(frames[0].payload["operation_id"], "operation-a");
        assert!(bus.since(frames[1].sequence).is_empty());
    }
}
