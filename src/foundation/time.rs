use std::time::{SystemTime, UNIX_EPOCH};

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn unix_now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn expires_at(expires_in: u64) -> u64 {
    unix_now().saturating_add(expires_in)
}

pub fn expires_at_millis(expires_in_secs: u64) -> u64 {
    unix_now_millis().saturating_add(expires_in_secs.saturating_mul(1000))
}

pub fn is_expired(expires_at: u64, margin_secs: u64) -> bool {
    unix_now().saturating_add(margin_secs) >= expires_at
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_is_in_the_future() {
        let at = expires_at(3600);
        assert!(at > unix_now());
        assert!(!is_expired(at, 60));
        assert!(is_expired(at, 3600));
    }

    #[test]
    fn millis_expiry_is_in_the_future() {
        let at = expires_at_millis(60);
        assert!(at > unix_now_millis());
        assert!(at - unix_now_millis() <= 60_000);
    }

    #[test]
    fn past_timestamp_is_expired() {
        assert!(is_expired(0, 0));
    }
}
