// key_rate.rs — Per-key sliding-window rate limiter for service API keys.
//
// Caps the blast radius of a leaked rover key. With these defaults, an
// attacker who somehow got a key off the rover can siphon at most ~60
// reads + 10 writes per minute, not unbounded.
//
// Cloudflare-migration note: once memoria moves to Workers (CLA-84), this
// is naturally replaced by Cloudflare's per-binding rate limiting, which
// is enforced at the edge and uses Durable Objects under the hood. The
// shape of the API we expose here matches what we'd want there: per-key
// buckets, read/write distinction, sliding window.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

/// Default per-key budget — sized well above the rover's 12s heartbeat
/// (which produces ~5 calls/min) so legitimate operation is unaffected,
/// but tight enough that a compromised key becomes a slow leak rather
/// than a fire-hose.
pub const READ_LIMIT_PER_MIN: usize = 60;
pub const WRITE_LIMIT_PER_MIN: usize = 10;
const WINDOW_MS: i64 = 60_000;

/// Epoch millis "now". chrono routes through `js_sys::Date::now()` on
/// wasm32 (via the `wasmbind` feature) and through the OS clock on
/// native — both work without invoking `std::time` which panics on
/// wasm32-unknown-unknown.
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Debug)]
pub struct RateLimited {
    pub is_write: bool,
}

#[derive(Debug, Default)]
struct Bucket {
    reads: VecDeque<i64>,
    writes: VecDeque<i64>,
}

impl Bucket {
    fn prune(buf: &mut VecDeque<i64>, now: i64) {
        while let Some(&front) = buf.front() {
            if now - front > WINDOW_MS {
                buf.pop_front();
            } else {
                break;
            }
        }
    }

    fn count_and_check(&mut self, is_write: bool) -> Result<(), RateLimited> {
        let now = now_ms();
        let (buf, limit) = if is_write {
            (&mut self.writes, WRITE_LIMIT_PER_MIN)
        } else {
            (&mut self.reads, READ_LIMIT_PER_MIN)
        };
        Self::prune(buf, now);
        if buf.len() >= limit {
            Err(RateLimited { is_write })
        } else {
            buf.push_back(now);
            Ok(())
        }
    }
}

#[derive(Debug, Default)]
pub struct KeyRateLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl KeyRateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a call by `key_id` and check it against the budget. Returns
    /// `Err(RateLimited)` if the call would exceed the limit (the call is
    /// NOT counted in that case — clients can retry after the window slides).
    pub fn check_and_count(&self, key_id: &str, is_write: bool) -> Result<(), RateLimited> {
        let mut buckets = self.buckets.lock().expect("rate limiter mutex poisoned");
        let bucket = buckets.entry(key_id.to_string()).or_default();
        bucket.count_and_check(is_write)
    }
}

/// Process-global limiter — lazy-initialised on first access so unit tests
/// in unrelated modules don't need to set it up explicitly.
static GLOBAL: OnceLock<KeyRateLimiter> = OnceLock::new();

pub fn global() -> &'static KeyRateLimiter {
    GLOBAL.get_or_init(KeyRateLimiter::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_limit_passes() {
        let rl = KeyRateLimiter::new();
        for _ in 0..READ_LIMIT_PER_MIN {
            rl.check_and_count("key-a", false).expect("under limit");
        }
        for _ in 0..WRITE_LIMIT_PER_MIN {
            rl.check_and_count("key-a", true).expect("under limit");
        }
    }

    #[test]
    fn over_read_limit_blocks() {
        let rl = KeyRateLimiter::new();
        for _ in 0..READ_LIMIT_PER_MIN {
            rl.check_and_count("key-a", false).unwrap();
        }
        let err = rl.check_and_count("key-a", false).unwrap_err();
        assert!(!err.is_write);
    }

    #[test]
    fn over_write_limit_blocks() {
        let rl = KeyRateLimiter::new();
        for _ in 0..WRITE_LIMIT_PER_MIN {
            rl.check_and_count("key-a", true).unwrap();
        }
        let err = rl.check_and_count("key-a", true).unwrap_err();
        assert!(err.is_write);
    }

    #[test]
    fn read_and_write_buckets_are_independent() {
        // Saturating the write bucket should not block reads from the same key.
        let rl = KeyRateLimiter::new();
        for _ in 0..WRITE_LIMIT_PER_MIN {
            rl.check_and_count("key-a", true).unwrap();
        }
        // Now writes are full — but reads should still work.
        rl.check_and_count("key-a", false)
            .expect("reads must not be affected by write-bucket saturation");
    }

    #[test]
    fn keys_have_independent_buckets() {
        let rl = KeyRateLimiter::new();
        for _ in 0..WRITE_LIMIT_PER_MIN {
            rl.check_and_count("key-a", true).unwrap();
        }
        // key-a is saturated for writes, key-b should be untouched.
        rl.check_and_count("key-b", true)
            .expect("distinct key_id should have its own bucket");
    }

    #[test]
    fn rate_limited_call_does_not_count() {
        // When a call is rejected, it should NOT push an entry into the
        // window — otherwise the bucket effectively never drains.
        let rl = KeyRateLimiter::new();
        for _ in 0..WRITE_LIMIT_PER_MIN {
            rl.check_and_count("key-a", true).unwrap();
        }
        // Hammer with denials — none of these should count.
        for _ in 0..10 {
            assert!(rl.check_and_count("key-a", true).is_err());
        }
        // Inspect: write deque length is still exactly WRITE_LIMIT_PER_MIN.
        let buckets = rl.buckets.lock().unwrap();
        let bucket = buckets.get("key-a").unwrap();
        assert_eq!(bucket.writes.len(), WRITE_LIMIT_PER_MIN);
    }
}
