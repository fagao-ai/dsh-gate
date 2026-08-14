use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// In-memory login sessions: id -> (username, expiry epoch seconds).
#[derive(Clone)]
pub struct SessionStore {
    inner: Arc<Mutex<HashMap<String, (String, u64)>>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub fn insert(&self, id: String, username: String, ttl: Duration) {
        let expiry = now() + ttl.as_secs();
        self.inner.lock().unwrap().insert(id, (username, expiry));
    }

    pub fn valid(&self, id: &str) -> bool {
        let now = now();
        let mut map = self.inner.lock().unwrap();
        match map.get(id) {
            Some((_, expiry)) if *expiry > now => true,
            Some(_) => { map.remove(id); false }
            None => false,
        }
    }

    pub fn remove(&self, id: &str) {
        self.inner.lock().unwrap().remove(id);
    }
}

/// One-time CSRF tokens for the login form: token -> expiry.
#[derive(Clone)]
pub struct CsrfStore {
    inner: Arc<Mutex<HashMap<String, u64>>>,
}

impl CsrfStore {
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub fn issue(&self, ttl: Duration) -> String {
        let token = random_hex(16);
        self.inner.lock().unwrap().insert(token.clone(), now() + ttl.as_secs());
        token
    }

    /// Consume a token; one-time use.
    pub fn consume(&self, token: &str) -> bool {
        let now = now();
        let mut map = self.inner.lock().unwrap();
        match map.remove(token) {
            Some(expiry) => expiry > now,
            None => false,
        }
    }
}

/// Per-IP login failure accounting: failures and lock-until.
#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<Mutex<HashMap<String, (u32, u64)>>>,
    max_failures: u32,
    lock_secs: u64,
}

impl RateLimiter {
    pub fn new(max_failures: u32, lock_secs: u64) -> Self {
        Self { inner: Arc::new(Mutex::new(HashMap::new())), max_failures, lock_secs }
    }

    /// Reset (after a successful login). Also reaps expired entries.
    pub fn reset(&self, ip: &str) {
        self.inner.lock().unwrap().remove(ip);
    }

    /// True when the IP is locked out right now.
    pub fn locked(&self, ip: &str) -> bool {
        let now = now();
        let map = self.inner.lock().unwrap();
        matches!(map.get(ip), Some((_, until)) if *until > now)
    }

    /// Record a failure; returns Some(seconds remaining) when the IP just
    /// crossed into a lockout, else None.
    pub fn record_failure(&self, ip: &str) -> Option<u64> {
        let mut map = self.inner.lock().unwrap();
        let now = now();
        let (count, _until) = map.get(ip).copied().unwrap_or((0, 0));
        let count = count + 1;
        if count >= self.max_failures {
            map.insert(ip.to_string(), (0, now + self.lock_secs));
            Some(self.lock_secs)
        } else {
            map.insert(ip.to_string(), (count, 0));
            None
        }
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

pub fn random_hex(bytes: usize) -> String {
    use rand::RngCore;
    let mut buf = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}
