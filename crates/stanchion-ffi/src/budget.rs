//! Per-instance call budget (static + runtime model for WASM / non-Lua paths).
//!
//! Mirrors `stanchion_registry::sandbox::Budget` but holds a copy in the entry
//! so call-time enforcement works without requiring the Lua state machinery.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Instruction/meters budget reset per call, enforced at the dispatch boundary.
#[derive(Debug, Clone)]
pub struct CallBudget {
    used: Arc<AtomicU64>,
    limit: u64,
}

impl CallBudget {
    pub fn new(limit: u64) -> Self {
        Self {
            used: Arc::new(AtomicU64::new(0)),
            limit,
        }
    }
    /// Reset before each call; matches `Budget::reset()`.
    pub fn reset(&self) {
        self.used.store(0, Ordering::Relaxed);
    }
    pub fn limit(&self) -> u64 { self.limit }
    pub fn used(&self) -> u64 { self.used.load(Ordering::Relaxed) }
    /// Charge `amount`; returns `Err` if over limit.
    pub fn consume(&self, amount: u64) -> Result<(), String> {
        let used = self.used.fetch_add(amount, Ordering::Relaxed).saturating_add(amount);
        if used > self.limit {
            return Err(format!("call exceeded budget of {} instructions", self.limit));
        }
        Ok(())
    }
}
