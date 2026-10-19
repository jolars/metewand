//! Local process accounting that never claims complete process-tree coverage.

use metewand_core::measurements::{Measurement, MissingReason, PartialReason};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ProcessUsage {
    cpu_time_ns: Option<u64>,
    peak_rss_bytes: Option<u64>,
}

impl ProcessUsage {
    pub(crate) fn observe(pid: u32) -> Self {
        #[cfg(target_os = "linux")]
        {
            let cpu_time_ns = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|stat| {
                    // SAFETY: sysconf reads a process-independent POSIX constant.
                    let ticks = u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) }).ok()?;
                    parse_cpu_time(&stat, ticks)
                });
            let peak_rss_bytes = std::fs::read_to_string(format!("/proc/{pid}/status"))
                .ok()
                .and_then(|status| parse_peak_rss(&status));
            Self {
                cpu_time_ns,
                peak_rss_bytes,
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = pid;
            Self::default()
        }
    }

    pub(crate) fn since_optional(self, baseline: Option<Self>) -> (Measurement, Measurement) {
        match baseline {
            Some(baseline) => self.since(baseline),
            None => (partial(self.cpu_time_ns), partial(self.peak_rss_bytes)),
        }
    }

    pub(crate) fn since(self, baseline: Self) -> (Measurement, Measurement) {
        let cpu = self
            .cpu_time_ns
            .zip(baseline.cpu_time_ns)
            .and_then(|(end, start)| end.checked_sub(start));
        (partial(cpu), partial(self.peak_rss_bytes))
    }
}

fn partial(value: Option<u64>) -> Measurement {
    match value {
        Some(value) if value <= 9_007_199_254_740_991 => Measurement::Partial {
            value,
            reason: PartialReason::ProcessOnly,
        },
        Some(_) => Measurement::unavailable(MissingReason::OutOfRange),
        None => Measurement::unavailable(if cfg!(target_os = "linux") {
            MissingReason::AccountingUnavailable
        } else {
            MissingReason::Unsupported
        }),
    }
}

#[cfg(target_os = "linux")]
fn parse_cpu_time(stat: &str, ticks_per_second: u64) -> Option<u64> {
    // comm can contain spaces and parentheses; fields after its last ')' are stable.
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
    let user = fields.nth(11)?.parse::<u64>().ok()?;
    let system = fields.next()?.parse::<u64>().ok()?;
    let nanos = u128::from(user.checked_add(system)?) * 1_000_000_000;
    u64::try_from(nanos.checked_div(u128::from(ticks_per_second))?).ok()
}

#[cfg(target_os = "linux")]
fn parse_peak_rss(status: &str) -> Option<u64> {
    let line = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?;
    let mut fields = line.split_whitespace();
    let kilobytes = fields.next()?.parse::<u64>().ok()?;
    (fields.next()? == "kB").then_some(())?;
    kilobytes.checked_mul(1024)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn process_accounting_excludes_child_ticks_and_handles_unusual_names() {
        let stat = "123 (worker ) (name)) S 1 2 3 4 5 6 7 8 9 10 20 30 900 800";
        assert_eq!(parse_cpu_time(stat, 100), Some(500_000_000));
        assert_eq!(parse_cpu_time(stat, 0), None);
        assert_eq!(parse_cpu_time("malformed", 100), None);
        assert_eq!(parse_peak_rss("VmRSS: 7 kB\nVmHWM: 42 kB\n"), Some(43008));
        assert_eq!(parse_peak_rss("VmHWM: 42 bytes"), None);
        assert_eq!(
            ProcessUsage::observe(u32::MAX)
                .since(ProcessUsage::default())
                .0,
            Measurement::unavailable(MissingReason::AccountingUnavailable)
        );
    }
}
