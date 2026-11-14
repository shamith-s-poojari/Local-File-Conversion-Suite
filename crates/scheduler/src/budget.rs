//! The memory budget (spec section 10.2).

use sysinfo::{MemoryRefreshKind, RefreshKind, System};

use crate::queue::MIN_HARD_LIMIT;

/// Reads how much memory is available. A trait so tests can fix the value.
pub trait MemoryProbe: Send + Sync {
    fn available_bytes(&self) -> u64;
}

/// The OS's available memory.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemMemory;

impl MemoryProbe for SystemMemory {
    fn available_bytes(&self) -> u64 {
        #[cfg(target_os = "macos")]
        if let Some(bytes) = macos_available() {
            return bytes;
        }
        let sys = System::new_with_specifics(
            RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
        );
        plausible_available(
            sys.available_memory(),
            sys.free_memory(),
            sys.total_memory(),
        )
    }
}

/// Memory macOS can hand out without paging anything in use: free,
/// inactive and speculative pages. sysinfo subtracts the compressor's pages
/// as well, which on a busy 8 GB Mac reads as 0 while gigabytes of inactive
/// pages could be reclaimed at once.
#[cfg(target_os = "macos")]
fn macos_available() -> Option<u64> {
    use std::sync::OnceLock;
    static HOST: OnceLock<libc::mach_port_t> = OnceLock::new();
    // SAFETY: returns this task's send right to the host port; kept for the
    // process's life, so it's taken once.
    #[allow(deprecated)]
    let host = *HOST.get_or_init(|| unsafe { libc::mach_host_self() });
    // SAFETY: zeroed is a valid vm_statistics64 (plain integers).
    let mut stats: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: `stats` holds HOST_VM_INFO64_COUNT integers, as `count` says.
    let rc = unsafe {
        libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            (&mut stats as *mut libc::vm_statistics64).cast(),
            &mut count,
        )
    };
    // SAFETY: plain sysconf query.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if rc != libc::KERN_SUCCESS || page <= 0 {
        return None;
    }
    let pages = u64::from(stats.free_count)
        + u64::from(stats.inactive_count)
        + u64::from(stats.speculative_count);
    Some(pages * page as u64)
}

/// Replaces an implausible "available" reading with a conservative one.
///
/// On macOS, sysinfo 0.37 computes available memory as
/// `free + inactive + purgeable - compressor pages`. When the compressor
/// holds more pages than the other three (common on 8 GB Macs), the result
/// saturates to 0, although the OS can still reclaim memory. A reading
/// above total RAM is also wrong. In either case, use free memory, but at
/// least 1/8 of total RAM, so one bad reading doesn't stall admission.
pub fn plausible_available(available: u64, free: u64, total: u64) -> u64 {
    if total == 0 {
        // Nothing to check against; `budget` applies its own floor.
        return available;
    }
    if available > 0 && available <= total {
        return available;
    }
    free.max(total / 8).min(total)
}

/// A fixed amount, for tests.
#[derive(Debug, Clone, Copy)]
pub struct FixedMemory(pub u64);

impl MemoryProbe for FixedMemory {
    fn available_bytes(&self) -> u64 {
        self.0
    }
}

/// `min(60% of available RAM, user setting)`.
///
/// `available` should include what running jobs have reserved: the OS
/// already counts their memory as used, and the scheduler subtracts their
/// reservations itself, so leaving it out would count them twice.
///
/// The 60% term is never below [`MIN_HARD_LIMIT`]. A 0 or tiny reading
/// would otherwise make every job "larger than the budget" and give it a
/// hard limit too small to start. The user setting still caps the result.
pub fn budget(available: u64, user_limit: Option<u64>) -> u64 {
    let sixty_percent = ((u128::from(available) * 6 / 10) as u64).max(MIN_HARD_LIMIT);
    match user_limit {
        Some(limit) => sixty_percent.min(limit),
        None => sixty_percent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;

    #[test]
    fn sixty_percent_capped_by_setting() {
        assert_eq!(budget(10 * GB, None), 6 * GB);
        assert_eq!(budget(10 * GB, Some(5 * GB)), 5 * GB);
        assert_eq!(budget(10 * GB, Some(9 * GB)), 6 * GB);
        assert_eq!(
            budget(u64::MAX, None),
            (u128::from(u64::MAX) * 6 / 10) as u64
        );
    }

    #[test]
    fn zero_or_tiny_reading_never_gives_zero_budget() {
        assert_eq!(budget(0, None), MIN_HARD_LIMIT);
        assert_eq!(budget(1000, None), MIN_HARD_LIMIT);
        assert_eq!(budget(0, Some(4 * GB)), MIN_HARD_LIMIT);
        // An explicit user setting below the floor is still honoured.
        assert_eq!(budget(0, Some(256 * MB)), 256 * MB);
    }

    #[test]
    fn plausible_reading_is_kept() {
        assert_eq!(plausible_available(3 * GB, GB, 8 * GB), 3 * GB);
        assert_eq!(plausible_available(8 * GB, 0, 8 * GB), 8 * GB);
    }

    #[test]
    fn zero_reading_falls_back_to_free_or_fraction_of_total() {
        // Free memory above 1/8 of total: use free.
        assert_eq!(plausible_available(0, 2 * GB, 8 * GB), 2 * GB);
        // The case seen on an 8 GB Mac: ~700 MB free, available read as 0.
        assert_eq!(plausible_available(0, 700 * MB, 8 * GB), GB);
        assert_eq!(plausible_available(0, 0, 8 * GB), GB);
    }

    #[test]
    fn reading_above_total_is_rejected() {
        assert_eq!(plausible_available(16 * GB, 2 * GB, 8 * GB), 2 * GB);
    }

    #[test]
    fn unknown_total_passes_reading_through() {
        assert_eq!(plausible_available(0, 0, 0), 0);
        assert_eq!(plausible_available(5, 0, 0), 5);
    }

    /// A probe that reads 0, like sysinfo on the affected Macs.
    struct ZeroMemory;

    impl MemoryProbe for ZeroMemory {
        fn available_bytes(&self) -> u64 {
            0
        }
    }

    #[test]
    fn zero_probe_still_admits_a_normal_job() {
        use crate::queue::{phase1_pools, Scheduler, Ticket};
        use converter_jobstore::JobId;
        use std::time::Instant;

        let probe: &dyn MemoryProbe = &ZeroMemory;
        let b = budget(probe.available_bytes(), None);
        assert!(b > 0);

        let now = Instant::now();
        let mut s = Scheduler::new(phase1_pools());
        s.enqueue(Ticket {
            job: JobId::parse("j_1").unwrap(),
            pool: "pdf".into(),
            estimate: 100 * MB,
            retry_alone: false,
            enqueued: now,
        });
        let a = s.admit(now, b);
        assert_eq!(a.len(), 1);
        assert!(!a[0].large);
        assert_eq!(a[0].hard_limit, MIN_HARD_LIMIT);
    }

    /// Whatever the machine's momentary memory state, the guarded reading
    /// is positive and no larger than total RAM.
    #[test]
    fn system_memory_reads_something() {
        let total = System::new_with_specifics(
            RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
        )
        .total_memory();
        let available = SystemMemory.available_bytes();
        assert!(total > 0, "sysinfo reports no RAM");
        assert!(available > 0);
        assert!(available <= total);
    }
}
