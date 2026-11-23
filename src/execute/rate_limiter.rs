//! Token-bucket read/write rate limiter (port of execute/rate_limiter.cpp).

use std::time::{Duration, Instant};

use crate::client::ConsumedCapacity;

pub type Clock = Box<dyn FnMut() -> f64>;
pub type Sleep = Box<dyn FnMut(f64)>;

pub struct RateLimiter {
    rcu: f64,
    wcu: f64,
    read_tokens: f64,
    write_tokens: f64,
    last: f64,
    clock: Clock,
    sleep: Sleep,
}

impl RateLimiter {
    pub fn new(rcu: f64, wcu: f64) -> RateLimiter {
        let origin = Instant::now();
        Self::with_clock(
            rcu,
            wcu,
            Box::new(move || origin.elapsed().as_secs_f64()),
            Box::new(|secs: f64| {
                if secs > 0.0 && secs.is_finite() {
                    std::thread::sleep(Duration::from_secs_f64(secs))
                }
            }),
        )
    }

    pub fn with_clock(rcu: f64, wcu: f64, mut clock: Clock, sleep: Sleep) -> RateLimiter {
        let last = clock();
        RateLimiter {
            rcu,
            wcu,
            read_tokens: 0.0,
            write_tokens: 0.0,
            last,
            clock,
            sleep,
        }
    }

    pub fn consume_reads(&mut self, cc: &Option<ConsumedCapacity>) {
        if let Some(cc) = cc {
            self.read_tokens -= cc.capacity_units.unwrap_or(0.0);
        }
    }

    pub fn consume_writes(&mut self, cc: &Option<ConsumedCapacity>) {
        if let Some(cc) = cc {
            self.write_tokens -= cc.capacity_units.unwrap_or(0.0);
        }
    }

    fn refresh(&mut self) {
        let now = (self.clock)();
        let elapsed = now - self.last;
        self.read_tokens = (self.read_tokens + elapsed * self.rcu).min(self.rcu);
        self.write_tokens = (self.write_tokens + elapsed * self.wcu).min(self.wcu);
        self.last = now;
    }

    pub fn readlimit(&mut self) {
        self.refresh();
        if self.rcu == 0.0 || self.read_tokens >= 0.0 {
            return;
        }
        loop {
            if self.read_tokens < 0.0 {
                (self.sleep)(-self.read_tokens / self.rcu);
            }
            self.refresh();
            if self.read_tokens >= 0.0 {
                break;
            }
        }
    }

    pub fn writelimit(&mut self) {
        self.refresh();
        if self.wcu == 0.0 || self.write_tokens >= 0.0 {
            return;
        }
        loop {
            if self.write_tokens < 0.0 {
                (self.sleep)(-self.write_tokens / self.wcu);
            }
            self.refresh();
            if self.write_tokens >= 0.0 {
                break;
            }
        }
    }
}
