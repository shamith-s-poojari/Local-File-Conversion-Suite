//! What reaches the renderer: job events, minus those of hidden jobs.
//!
//! A hidden job (a file worker's) is only known to be hidden once the store
//! has returned its ID, but the store delivers the `created` event before it
//! returns. So a hidden job is created through [`Events::create_hidden`],
//! which holds every event back while the store call runs, registers the new
//! ID, and then releases whatever isn't hidden. Other jobs' events are
//! delayed by the length of one directory creation, never lost or reordered
//! among themselves.

use std::collections::HashSet;
use std::sync::{Mutex, RwLock};

use converter_jobstore::{JobError, JobEvent, JobId, JobStore};

use crate::{lock, Sink};

#[derive(Default)]
struct Gate {
    /// Hidden jobs being created right now.
    creating: usize,
    held: Vec<JobEvent>,
}

pub(crate) struct Events {
    sink: RwLock<Sink>,
    hidden: Mutex<HashSet<JobId>>,
    gate: Mutex<Gate>,
}

impl Events {
    pub fn new(sink: Sink) -> Self {
        Events {
            sink: RwLock::new(sink),
            hidden: Mutex::default(),
            gate: Mutex::default(),
        }
    }

    /// The store's event callback.
    pub fn on_event(&self, e: &JobEvent) {
        {
            let mut gate = lock(&self.gate);
            if gate.creating > 0 {
                gate.held.push(e.clone());
                return;
            }
        }
        self.deliver(e);
    }

    fn deliver(&self, e: &JobEvent) {
        if lock(&self.hidden).contains(&e.job_id) {
            return;
        }
        let f = self.sink.read().unwrap_or_else(|p| p.into_inner()).clone();
        f(e);
    }

    /// Creates a job whose events never reach the sink, its `created` event
    /// included.
    pub fn create_hidden(&self, store: &JobStore) -> Result<JobId, JobError> {
        lock(&self.gate).creating += 1;
        let created = store.create_job().map(|(id, _)| id);
        if let Ok(id) = &created {
            lock(&self.hidden).insert(id.clone());
        }
        let released = {
            let mut gate = lock(&self.gate);
            gate.creating -= 1;
            if gate.creating == 0 {
                std::mem::take(&mut gate.held)
            } else {
                Vec::new()
            }
        };
        for e in &released {
            self.deliver(e);
        }
        created
    }

    pub fn unhide(&self, job: &JobId) {
        lock(&self.hidden).remove(job);
    }

    pub fn hidden_jobs(&self) -> HashSet<JobId> {
        lock(&self.hidden).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use converter_jobstore::{DataRoot, StoreConfig, SyncRoots};
    use std::sync::Arc;

    fn setup() -> (
        tempfile::TempDir,
        Arc<Events>,
        JobStore,
        Arc<Mutex<Vec<JobId>>>,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let seen: Arc<Mutex<Vec<JobId>>> = Arc::default();
        let sink: Sink = {
            let seen = seen.clone();
            Arc::new(move |e: &JobEvent| lock(&seen).push(e.job_id.clone()))
        };
        let events = Arc::new(Events::new(sink));
        let root = DataRoot::open(&tmp.path().join("data"), &SyncRoots::none()).unwrap();
        let filter = {
            let events = events.clone();
            Arc::new(move |e: &JobEvent| events.on_event(e))
        };
        let (store, _) = JobStore::open(
            root,
            SyncRoots::none(),
            StoreConfig {
                on_event: Some(filter),
                ..StoreConfig::default()
            },
        )
        .unwrap();
        (tmp, events, store, seen)
    }

    #[test]
    fn a_hidden_jobs_created_event_never_reaches_the_sink() {
        let (_tmp, events, store, seen) = setup();
        let hidden = events.create_hidden(&store).unwrap();
        let (visible, _) = store.create_job().unwrap();
        assert_eq!(*lock(&seen), vec![visible.clone()]);
        assert!(events.hidden_jobs().contains(&hidden));
        // Later events of the hidden job stay quiet too.
        store
            .fail(
                &hidden,
                converter_jobstore::JobFailure {
                    code: "E_CANCELLED".into(),
                    retryable: false,
                },
            )
            .unwrap();
        assert_eq!(*lock(&seen), vec![visible]);
    }
}
