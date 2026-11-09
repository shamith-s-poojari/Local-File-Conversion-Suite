//! The memory budget (spec section 10.2).

use sysinfo::{MemoryRefreshKind, RefreshKind, System};

/// Reads how much memory is available. A trait so tests can fix the value.
pub trait MemoryProbe: Send + Sync {
    fn available_bytes(&self) -> u64;
}

/// The OS's available memory.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemMemory;

impl MemoryProbe for SystemMemory {
    fn available_bytes(&self) -> u64 {
        let sys = System::new_with_specifics(
            RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
        );
        sys.available_memory()
    }
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
pub fn budget(available: u64, user_limit: Option<u64>) -> u64 {
    let sixty_percent = (u128::from(available) * 6 / 10) as u64;
    match user_limit {
        Some(limit) => sixty_percent.min(limit),
        None => sixty_percent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixty_percent_capped_by_setting() {
        assert_eq!(budget(10_000, None), 6_000);
        assert_eq!(budget(10_000, Some(5_000)), 5_000);
        assert_eq!(budget(10_000, Some(9_000)), 6_000);
        assert_eq!(
            budget(u64::MAX, None),
            (u128::from(u64::MAX) * 6 / 10) as u64
        );
    }

    #[test]
    fn system_memory_reads_something() {
        assert!(SystemMemory.available_bytes() > 0);
    }
}
