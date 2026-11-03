//! Bounded process-local login admission. Forwarded headers are deliberately ignored.
use anyhow::{Result, ensure};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

#[derive(Clone, Copy)]
pub struct LoginLimitConfig {
    pub window_seconds: u64,
    pub per_account: u32,
    pub global: u32,
    pub max_keys: usize,
}
impl Default for LoginLimitConfig {
    fn default() -> Self {
        Self {
            window_seconds: 60,
            per_account: 10,
            global: 300,
            max_keys: 1024,
        }
    }
}
#[derive(Clone)]
pub struct LoginLimiter {
    config: LoginLimitConfig,
    state: Arc<Mutex<State>>,
}
struct State {
    since: Instant,
    total: u32,
    keys: HashMap<String, (Instant, u32)>,
}
impl Default for LoginLimiter {
    fn default() -> Self {
        Self::new(LoginLimitConfig::default()).expect("valid defaults")
    }
}
impl LoginLimiter {
    pub fn new(config: LoginLimitConfig) -> Result<Self> {
        ensure!(
            (1..=3600).contains(&config.window_seconds),
            "login window must be 1..3600 seconds"
        );
        ensure!(
            config.per_account > 0 && config.global > 0,
            "login limits must be positive"
        );
        ensure!(
            (1..=100_000).contains(&config.max_keys),
            "login key limit must be 1..100000"
        );
        Ok(Self {
            config,
            state: Arc::new(Mutex::new(State {
                since: Instant::now(),
                total: 0,
                keys: HashMap::new(),
            })),
        })
    }
    pub async fn allow(&self, account: &str) -> bool {
        self.allow_at(account, Instant::now()).await
    }
    async fn allow_at(&self, account: &str, now: Instant) -> bool {
        let mut state = self.state.lock().await;
        let window = Duration::from_secs(self.config.window_seconds);
        if now.duration_since(state.since) >= window {
            state.since = now;
            state.total = 0;
        }
        if state.total >= self.config.global {
            return false;
        }
        state.total += 1;
        state
            .keys
            .retain(|_, (since, _)| now.duration_since(*since) < window);
        if state.keys.len() >= self.config.max_keys && !state.keys.contains_key(account) {
            return false;
        }
        let entry = state.keys.entry(account.to_owned()).or_insert((now, 0));
        if entry.1 >= self.config.per_account {
            return false;
        }
        entry.1 += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn limits_are_shared_bounded_and_recover_without_sleep() {
        let limiter = LoginLimiter::new(LoginLimitConfig {
            window_seconds: 60,
            per_account: 2,
            global: 4,
            max_keys: 2,
        })
        .unwrap();
        let now = Instant::now();
        assert!(limiter.allow_at("one", now).await);
        assert!(limiter.clone().allow_at("one", now).await);
        assert!(!limiter.allow_at("one", now).await);
        assert!(limiter.allow_at("two", now).await);
        assert!(!limiter.allow_at("three", now).await);
        assert!(
            limiter
                .allow_at("three", now + Duration::from_secs(60))
                .await
        );
        assert!(
            LoginLimiter::new(LoginLimitConfig {
                max_keys: 0,
                ..Default::default()
            })
            .is_err()
        );
    }
}
