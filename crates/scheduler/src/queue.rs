//! Admission against the memory budget (spec section 10.2).
//!
//! This is a pure data structure: the caller supplies the time and the
//! budget, so every rule can be tested without processes or clocks.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use converter_jobstore::JobId;

/// Smallest hard memory limit a job gets.
pub const MIN_HARD_LIMIT: u64 = 512 * 1024 * 1024;

/// After this long in the queue, a job that's skipped blocks every job
/// behind it, so the jobs ahead drain and it can run.
pub const STARVATION_AFTER: Duration = Duration::from_secs(60);

/// Phase 1 pool sizes: PDF structural 2, render 2, redact 1. Pools not
/// listed get one slot.
pub fn phase1_pools() -> HashMap<String, usize> {
    [("pdf", 2), ("render", 2), ("redact", 1)]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect()
}

/// A job's hard memory limit: `min(max(1.5 × estimate, 512 MB), budget)`.
pub fn hard_limit(estimate: u64, budget: u64) -> u64 {
    let scaled = estimate.saturating_add(estimate / 2);
    scaled.max(MIN_HARD_LIMIT).min(budget)
}

/// A job waiting for admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub job: JobId,
    pub pool: String,
    /// The worker's `ramBytes` estimate.
    pub estimate: u64,
    /// The memory retry: run alone, with the whole budget as the limit.
    pub retry_alone: bool,
    pub enqueued: Instant,
}

/// A job that may start now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    pub job: JobId,
    /// Memory the sandbox must enforce for this job.
    pub hard_limit: u64,
    /// The estimate exceeds the whole budget: the job runs alone and gets
    /// `W_LARGE_JOB`.
    pub large: bool,
}

#[derive(Debug, Clone)]
struct Slot {
    pool: String,
    reserved: u64,
    alone: bool,
}

/// The admission queue and the set of running jobs.
#[derive(Debug, Clone)]
pub struct Scheduler {
    pools: HashMap<String, usize>,
    starvation: Duration,
    queue: Vec<Ticket>,
    running: HashMap<JobId, Slot>,
}

impl Scheduler {
    pub fn new(pools: HashMap<String, usize>) -> Self {
        Self::with_starvation(pools, STARVATION_AFTER)
    }

    pub fn with_starvation(pools: HashMap<String, usize>, starvation: Duration) -> Self {
        Scheduler {
            pools,
            starvation,
            queue: Vec::new(),
            running: HashMap::new(),
        }
    }

    /// Adds a job to the back of the queue. A job already queued or running
    /// is ignored.
    pub fn enqueue(&mut self, ticket: Ticket) {
        if self.is_queued(&ticket.job) || self.running.contains_key(&ticket.job) {
            return;
        }
        self.queue.push(ticket);
    }

    /// Removes a queued job, for example on cancel. True if it was queued.
    pub fn dequeue(&mut self, job: &JobId) -> bool {
        let before = self.queue.len();
        self.queue.retain(|t| &t.job != job);
        before != self.queue.len()
    }

    /// Releases a running job's slot and reservation.
    pub fn finish(&mut self, job: &JobId) {
        self.running.remove(job);
    }

    pub fn is_queued(&self, job: &JobId) -> bool {
        self.queue.iter().any(|t| &t.job == job)
    }

    pub fn is_running(&self, job: &JobId) -> bool {
        self.running.contains_key(job)
    }

    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    pub fn running(&self) -> usize {
        self.running.len()
    }

    /// Memory reserved by running jobs.
    pub fn reserved(&self) -> u64 {
        self.running.values().map(|s| s.reserved).sum()
    }

    /// Admits every job that may start now, in queue order.
    ///
    /// - A job runs if its pool has a free slot and its hard limit fits in
    ///   what's left of `budget`.
    /// - A job whose estimate exceeds the whole budget, or that's on its
    ///   memory retry, runs alone: only when nothing else runs, and nothing
    ///   else starts while it runs. Its limit is the whole budget.
    /// - A job that doesn't fit is skipped, unless it has waited longer than
    ///   the starvation time; then it blocks every job behind it.
    pub fn admit(&mut self, now: Instant, budget: u64) -> Vec<Admission> {
        let mut admitted = Vec::new();
        let mut i = 0;
        while i < self.queue.len() {
            if self.running.values().any(|s| s.alone) {
                break;
            }
            let t = &self.queue[i];
            let cap = self.pools.get(&t.pool).copied().unwrap_or(1);
            let in_pool = self.running.values().filter(|s| s.pool == t.pool).count();
            if in_pool >= cap {
                // Later jobs in this pool can't start either, so this keeps
                // pool order; jobs in other pools may go ahead.
                i += 1;
                continue;
            }
            let large = t.estimate > budget;
            let alone = large || t.retry_alone;
            let limit = if alone {
                budget
            } else {
                hard_limit(t.estimate, budget)
            };
            let fits = if alone {
                self.running.is_empty()
            } else {
                self.reserved().saturating_add(limit) <= budget
            };
            if fits {
                let t = self.queue.remove(i);
                self.running.insert(
                    t.job.clone(),
                    Slot {
                        pool: t.pool,
                        reserved: limit,
                        alone,
                    },
                );
                admitted.push(Admission {
                    job: t.job,
                    hard_limit: limit,
                    large,
                });
                continue;
            }
            if now.saturating_duration_since(t.enqueued) >= self.starvation {
                break;
            }
            i += 1;
        }
        admitted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;

    fn id(n: u32) -> JobId {
        JobId::parse(&format!("j_{n}")).unwrap()
    }

    fn ticket(n: u32, pool: &str, estimate: u64, at: Instant) -> Ticket {
        Ticket {
            job: id(n),
            pool: pool.into(),
            estimate,
            retry_alone: false,
            enqueued: at,
        }
    }

    fn ids(a: &[Admission]) -> Vec<JobId> {
        a.iter().map(|a| a.job.clone()).collect()
    }

    #[test]
    fn hard_limit_formula() {
        assert_eq!(hard_limit(0, 8 * GB), MIN_HARD_LIMIT);
        assert_eq!(hard_limit(400 * MB, 8 * GB), 600 * MB);
        assert_eq!(hard_limit(6 * GB, 8 * GB), 8 * GB);
        assert_eq!(hard_limit(10 * MB, 100 * MB), 100 * MB);
        assert_eq!(hard_limit(u64::MAX, 8 * GB), 8 * GB);
    }

    #[test]
    fn pools_cap_concurrency() {
        let t0 = Instant::now();
        let mut s = Scheduler::new(phase1_pools());
        for n in 0..3 {
            s.enqueue(ticket(n, "pdf", 0, t0));
        }
        s.enqueue(ticket(3, "redact", 0, t0));
        s.enqueue(ticket(4, "redact", 0, t0));
        let a = s.admit(t0, 64 * GB);
        assert_eq!(ids(&a), [id(0), id(1), id(3)]);
        s.finish(&id(0));
        assert_eq!(ids(&s.admit(t0, 64 * GB)), [id(2)]);
        s.finish(&id(3));
        assert_eq!(ids(&s.admit(t0, 64 * GB)), [id(4)]);
    }

    #[test]
    fn first_fit_skips_until_starved() {
        let t0 = Instant::now();
        let mut s = Scheduler::new(phase1_pools());
        s.enqueue(ticket(0, "pdf", 2 * GB, t0)); // limit 3 GB
        s.admit(t0, 4 * GB);
        s.enqueue(ticket(1, "render", 2 * GB, t0)); // 3 GB: doesn't fit
        s.enqueue(ticket(2, "render", 100 * MB, t0)); // 512 MB: fits
        assert_eq!(ids(&s.admit(t0, 4 * GB)), [id(2)]);

        // Once job 1 is starved, job 3 behind it must wait.
        s.enqueue(ticket(3, "redact", 0, t0));
        let later = t0 + STARVATION_AFTER;
        assert!(s.admit(later, 4 * GB).is_empty());
        s.finish(&id(0));
        s.finish(&id(2));
        assert_eq!(ids(&s.admit(later, 4 * GB)), [id(1), id(3)]);
    }

    #[test]
    fn large_job_runs_alone() {
        let t0 = Instant::now();
        let mut s = Scheduler::new(phase1_pools());
        s.enqueue(ticket(0, "pdf", 0, t0));
        s.admit(t0, 4 * GB);
        s.enqueue(ticket(1, "pdf", 5 * GB, t0));
        s.enqueue(ticket(2, "render", 0, t0));
        // The large job waits for job 0; job 2 may skip ahead while it's
        // not starved.
        assert_eq!(ids(&s.admit(t0, 4 * GB)), [id(2)]);
        s.finish(&id(0));
        s.finish(&id(2));
        let a = s.admit(t0, 4 * GB);
        assert_eq!(a.len(), 1);
        assert!(a[0].large);
        assert_eq!(a[0].hard_limit, 4 * GB);
        // Nothing starts beside it.
        s.enqueue(ticket(3, "render", 0, t0));
        assert!(s.admit(t0, 4 * GB).is_empty());
        s.finish(&id(1));
        assert_eq!(ids(&s.admit(t0, 4 * GB)), [id(3)]);
    }

    #[test]
    fn memory_retry_gets_the_whole_budget_alone() {
        let t0 = Instant::now();
        let mut s = Scheduler::new(phase1_pools());
        s.enqueue(Ticket {
            retry_alone: true,
            ..ticket(0, "pdf", 100 * MB, t0)
        });
        let a = s.admit(t0, 6 * GB);
        assert_eq!(a[0].hard_limit, 6 * GB);
        assert!(!a[0].large);
        s.enqueue(ticket(1, "pdf", 0, t0));
        assert!(s.admit(t0, 6 * GB).is_empty());
    }

    #[test]
    fn dequeue_and_duplicates() {
        let t0 = Instant::now();
        let mut s = Scheduler::new(phase1_pools());
        s.enqueue(ticket(0, "pdf", 0, t0));
        s.enqueue(ticket(0, "pdf", 0, t0));
        assert_eq!(s.queued(), 1);
        assert!(s.dequeue(&id(0)));
        assert!(!s.dequeue(&id(0)));
        assert!(s.admit(t0, 64 * GB).is_empty());
    }

    /// Random operations never break the invariants: pools stay within
    /// capacity, reservations within the budget unless a job runs alone, and
    /// an alone job never shares.
    #[test]
    fn invariants_hold_under_random_operations() {
        let pools = phase1_pools();
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut rand = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let t0 = Instant::now();
        let budget = 4 * GB;
        let mut s = Scheduler::new(pools.clone());
        let mut alone_running: Option<JobId> = None;
        let mut next = 0u32;
        for step in 0..5000u64 {
            let now = t0 + Duration::from_secs(step / 10);
            match rand(4) {
                0 | 1 => {
                    let pool = ["pdf", "render", "redact"][rand(3) as usize];
                    let mut t = ticket(next, pool, rand(6) * GB / 2, now);
                    t.retry_alone = rand(20) == 0;
                    s.enqueue(t);
                    next += 1;
                }
                2 => {
                    if let Some(job) = s.running.keys().next().cloned() {
                        if alone_running.as_ref() == Some(&job) {
                            alone_running = None;
                        }
                        s.finish(&job);
                    }
                }
                _ => {
                    for a in s.admit(now, budget) {
                        if s.running[&a.job].alone {
                            alone_running = Some(a.job.clone());
                        }
                    }
                }
            }
            for (pool, cap) in &pools {
                let n = s.running.values().filter(|r| &r.pool == pool).count();
                assert!(n <= *cap, "pool {pool} over capacity");
            }
            if alone_running.is_some() {
                assert_eq!(s.running(), 1, "alone job shares");
            } else {
                assert!(s.reserved() <= budget, "over budget");
            }
        }
    }
}
