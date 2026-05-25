use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

/// Single-session-per-user lock. The `Guard` returned by `acquire` should be
/// held for the lifetime of the user's active connection. If a new acquire
/// arrives, the previous guard's `kicked()` future resolves, signalling the
/// owner to drop the connection.
#[derive(Clone, Default)]
pub struct SessionLock {
    inner: Arc<Mutex<HashMap<i64, Arc<Slot>>>>,
}

struct Slot {
    kicked: Notify,
}

pub struct Guard {
    user_id: i64,
    slot: Arc<Slot>,
    inner: Arc<Mutex<HashMap<i64, Arc<Slot>>>>,
}

impl Guard {
    pub async fn kicked(&self) {
        self.slot.kicked.notified().await;
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let inner = self.inner.clone();
        let uid = self.user_id;
        let slot = self.slot.clone();
        // Only remove the entry if it's still us.
        tokio::spawn(async move {
            let mut g = inner.lock().await;
            if let Some(existing) = g.get(&uid) {
                if Arc::ptr_eq(existing, &slot) {
                    g.remove(&uid);
                }
            }
        });
    }
}

impl SessionLock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquire the lock for `user_id`. Returns a Guard immediately; if an
    /// existing session was held, it gets kicked.
    pub async fn acquire(&self, user_id: i64) -> Guard {
        let new_slot = Arc::new(Slot {
            kicked: Notify::new(),
        });
        let mut g = self.inner.lock().await;
        if let Some(prev) = g.insert(user_id, new_slot.clone()) {
            prev.kicked.notify_waiters();
        }
        Guard {
            user_id,
            slot: new_slot,
            inner: self.inner.clone(),
        }
    }

    /// Kick the current holder (if any) without taking over the slot.
    pub async fn kick(&self, user_id: i64) {
        let mut g = self.inner.lock().await;
        if let Some(prev) = g.remove(&user_id) {
            prev.kicked.notify_waiters();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn second_acquire_kicks_first() {
        let lock = SessionLock::new();
        let g1 = lock.acquire(42).await;
        // Spawn a watcher on the first guard.
        let kicked_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let kf = kicked_flag.clone();
        let task = tokio::spawn(async move {
            g1.kicked().await;
            kf.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        // Give the watcher a moment to subscribe to the notifier.
        tokio::time::sleep(Duration::from_millis(20)).await;
        // Second acquire by same user_id should kick the first.
        let _g2 = lock.acquire(42).await;
        // Wait briefly and verify.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(kicked_flag.load(std::sync::atomic::Ordering::SeqCst));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn different_users_dont_collide() {
        let lock = SessionLock::new();
        let g1 = lock.acquire(1).await;
        let kicked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let kf = kicked.clone();
        tokio::spawn(async move {
            g1.kicked().await;
            kf.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let _g2 = lock.acquire(2).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!kicked.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn explicit_kick_signals() {
        let lock = SessionLock::new();
        let g1 = lock.acquire(7).await;
        let kicked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let kf = kicked.clone();
        tokio::spawn(async move {
            g1.kicked().await;
            kf.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        lock.kick(7).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(kicked.load(std::sync::atomic::Ordering::SeqCst));
    }
}
