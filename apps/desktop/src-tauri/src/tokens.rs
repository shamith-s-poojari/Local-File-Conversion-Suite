//! One-time tokens the renderer holds in place of things Rust keeps: a save
//! waiting for the user to confirm replacing an input, and the place a result
//! was saved. The renderer never sees a path (spec 11.1), so "confirm",
//! "cancel" and "show in folder" name what they mean by a token that Rust
//! checks.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use converter_jobstore::SaveDestination;

/// How long a saved result can still be shown in the file manager.
pub const REVEAL_TTL: Duration = Duration::from_secs(10 * 60);
/// Most reveal tokens kept: a long session of saves can't grow the map.
const MAX_REVEALS: usize = 64;

/// 128 random bits as hex.
pub fn random_token() -> String {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).expect("the OS random number generator is unavailable");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Save destinations waiting for the user to confirm overwriting an input
/// (audit S16, R18). At most one per job; the token goes to the renderer and
/// must come back with the confirmation or the cancel.
#[derive(Default)]
pub struct PendingSaves {
    by_job: Mutex<HashMap<String, (String, SaveDestination)>>,
}

impl PendingSaves {
    /// Holds `dest` for `job` (replacing an earlier one) and returns its token.
    pub fn hold(&self, job: &str, dest: SaveDestination) -> String {
        let token = random_token();
        self.lock().insert(job.to_owned(), (token.clone(), dest));
        token
    }

    /// The held destination, once: only with the token `hold` gave. A wrong
    /// token leaves the held one alone.
    pub fn take(&self, job: &str, token: &str) -> Option<SaveDestination> {
        let mut held = self.lock();
        if held.get(job).is_some_and(|(t, _)| t == token) {
            return held.remove(job).map(|(_, dest)| dest);
        }
        None
    }

    /// Drops the held destination for `job`, if `token` is its token.
    pub fn cancel(&self, job: &str, token: &str) -> bool {
        self.take(job, token).is_some()
    }

    /// The job was discarded or saved: nothing can be pending for it.
    pub fn clear_job(&self, job: &str) {
        self.lock().remove(job);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (String, SaveDestination)>> {
        self.by_job.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Where results were saved, for "Show in folder". A token works once and
/// for [`REVEAL_TTL`].
#[derive(Default)]
pub struct RevealTokens {
    paths: Mutex<HashMap<String, (PathBuf, Instant)>>,
}

impl RevealTokens {
    pub fn issue(&self, path: PathBuf) -> String {
        self.issue_at(path, Instant::now())
    }

    pub fn take(&self, token: &str) -> Option<PathBuf> {
        self.take_at(token, Instant::now())
    }

    fn issue_at(&self, path: PathBuf, now: Instant) -> String {
        let token = random_token();
        let mut paths = self.lock();
        paths.retain(|_, (_, at)| now.saturating_duration_since(*at) < REVEAL_TTL);
        while paths.len() >= MAX_REVEALS {
            let Some(oldest) = paths
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            paths.remove(&oldest);
        }
        paths.insert(token.clone(), (path, now));
        token
    }

    fn take_at(&self, token: &str, now: Instant) -> Option<PathBuf> {
        let (path, at) = self.lock().remove(token)?;
        (now.saturating_duration_since(at) < REVEAL_TTL).then_some(path)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (PathBuf, Instant)>> {
        self.paths.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(name: &str) -> SaveDestination {
        SaveDestination::File(PathBuf::from(name))
    }

    #[test]
    fn a_pending_save_is_confirmed_once_with_its_token() {
        let p = PendingSaves::default();
        let token = p.hold("job1", dest("a"));
        assert_eq!(token.len(), 32);
        assert!(p.take("job1", "wrong").is_none());
        assert!(p.take("job2", &token).is_none(), "another job's token");
        let held = p.take("job1", &token);
        assert!(matches!(held, Some(SaveDestination::File(f)) if f.to_str() == Some("a")));
        assert!(p.take("job1", &token).is_none(), "one time only");
    }

    #[test]
    fn holding_again_replaces_the_old_token() {
        let p = PendingSaves::default();
        let first = p.hold("job1", dest("a"));
        let second = p.hold("job1", dest("b"));
        assert_ne!(first, second);
        assert!(p.take("job1", &first).is_none());
        assert!(p.take("job1", &second).is_some());
    }

    #[test]
    fn cancel_needs_the_token_and_clears_it() {
        let p = PendingSaves::default();
        let token = p.hold("job1", dest("a"));
        assert!(!p.cancel("job1", "wrong"));
        assert!(p.cancel("job1", &token));
        assert!(p.take("job1", &token).is_none());
    }

    #[test]
    fn discarding_a_job_clears_its_pending_save() {
        let p = PendingSaves::default();
        let token = p.hold("job1", dest("a"));
        let other = p.hold("job2", dest("b"));
        p.clear_job("job1");
        assert!(p.take("job1", &token).is_none());
        assert!(p.take("job2", &other).is_some(), "other jobs are untouched");
    }

    #[test]
    fn a_reveal_token_works_once() {
        let r = RevealTokens::default();
        let token = r.issue(PathBuf::from("/saved/a.pdf"));
        assert!(r.take("0000").is_none());
        assert_eq!(r.take(&token), Some(PathBuf::from("/saved/a.pdf")));
        assert_eq!(r.take(&token), None);
    }

    #[test]
    fn a_reveal_token_expires() {
        let r = RevealTokens::default();
        let start = Instant::now();
        let token = r.issue_at(PathBuf::from("/saved/a.pdf"), start);
        let late = start + REVEAL_TTL + Duration::from_secs(1);
        assert_eq!(r.take_at(&token, late), None);
        // An expired token is gone, not just refused.
        assert_eq!(r.take_at(&token, start), None);

        let fresh = r.issue_at(PathBuf::from("/saved/b.pdf"), start);
        assert_eq!(
            r.take_at(&fresh, start + REVEAL_TTL - Duration::from_secs(1)),
            Some(PathBuf::from("/saved/b.pdf"))
        );
    }

    #[test]
    fn reveal_tokens_are_bounded_and_expired_ones_pruned() {
        let r = RevealTokens::default();
        let start = Instant::now();
        let old = r.issue_at(PathBuf::from("/old"), start);
        for i in 0..MAX_REVEALS + 10 {
            r.issue_at(
                PathBuf::from(format!("/p{i}")),
                start + REVEAL_TTL + Duration::from_secs(1),
            );
        }
        assert!(r.lock().len() <= MAX_REVEALS);
        assert_eq!(r.take_at(&old, start), None, "pruned");
    }
}
