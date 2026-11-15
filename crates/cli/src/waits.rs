//! How long this process waits out a production bound: a command's timeout,
//! a grace, the pause before a retry. Every such bound a test waits out
//! passes through `bound`, which is the bound itself, unless the process was
//! started with `AFKCONVOY_TEST_SECOND_MS`: the milliseconds one second of
//! every bound then takes. A test world says it, and so a test of "times
//! out", "retries" or "waits for the grace" takes milliseconds of real time,
//! not the production bound (sunfmin/control#1157). No host says it, and
//! what is said to a human keeps the production bound's seconds.
//!
//! It scales a wait, never a poll: how often a loop looks again is not
//! what a test waits out, and looking more often only loads the host.

use std::sync::LazyLock;
use std::time::Duration;

/// The name of the test seam: milliseconds a second of a bound takes.
pub const TEST_SECOND_MS: &str = "AFKCONVOY_TEST_SECOND_MS";

/// The milliseconds one second of a bound takes in this process, read once:
/// 1000 unless a test said otherwise.
fn second_ms() -> u64 {
    static SAID: LazyLock<u64> = LazyLock::new(|| {
        std::env::var(TEST_SECOND_MS).ok().and_then(|ms| ms.parse().ok()).unwrap_or(1000)
    });
    *SAID
}

/// `production`, as this process waits it out.
pub fn bound(production: Duration) -> Duration {
    scaled(production, second_ms())
}

fn scaled(production: Duration, second_ms: u64) -> Duration {
    if second_ms == 1000 {
        return production;
    }
    let nanos = production.as_nanos().saturating_mul(u128::from(second_ms)) / 1000;
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bound_is_whole_unless_a_test_scales_its_seconds() {
        let twenty = Duration::from_secs(20);
        assert_eq!(scaled(twenty, 1000), twenty);
        assert_eq!(scaled(twenty, 50), Duration::from_secs(1));
        assert_eq!(scaled(Duration::from_millis(250), 4), Duration::from_millis(1));
        assert_eq!(scaled(twenty, 0), Duration::ZERO);
    }
}
