//! The 30-second TOTP window: how long to wait for the next code.

pub const TOTP_PERIOD_SECS: u64 = 30;

/// Wait for the next code window, with one second for authenticator rollover.
pub fn secs_until_next_totp_window(now_unix: u64) -> u64 {
    TOTP_PERIOD_SECS - now_unix % TOTP_PERIOD_SECS + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case(0, 31)]
    #[case(30, 31)]
    #[case(45, 16)]
    #[case(59, 2)]
    fn totp_window_wait(#[case] now: u64, #[case] expected: u64) {
        assert_eq!(secs_until_next_totp_window(now), expected);
    }
}
