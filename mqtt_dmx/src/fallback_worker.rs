//! One explicit fallback save at a time, off the lighting loop. Blocking work is
//! owned by the managed service's Tokio runtime; cancellation cannot interrupt an
//! OS filesystem call. The service must enforce its existing process stop ceiling.
use crate::{
    recovery::Snapshot,
    recovery_file::{Error as FileError, FallbackFile},
};
use dmx_protocol::{Error as ProtocolError, Id, Revision};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("another fallback save is still running")]
    Busy,
    #[error("fallback worker is closed")]
    Closed,
    #[error("fallback save cancelled or expired; inspect the file and save id")]
    OutcomeUnknown,
    #[error("fallback worker unavailable: {0}")]
    Unavailable(String),
    #[error(transparent)]
    Document(#[from] ProtocolError),
    #[error(transparent)]
    File(#[from] FileError),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub save_id: Id,
    pub revision: Revision,
    pub digest: String,
}
#[derive(Debug)]
struct Job {
    cancelled: AtomicBool,
    started: Instant,
    deadline: Option<Instant>,
}
impl Job {
    fn allowed(&self) -> bool {
        !self.cancelled.load(Ordering::Acquire) && self.deadline.is_none_or(|d| Instant::now() < d)
    }
}
type Sink = Box<dyn FnMut(Id, &str, &dyn Fn() -> bool) -> Result<(), FileError> + Send>;
struct Inner {
    sink: Mutex<Sink>,
    slots: Arc<Semaphore>,
    closed: AtomicBool,
    active: Mutex<Option<Arc<Job>>>,
}
pub struct SaveWorker {
    inner: Arc<Inner>,
}
#[derive(Debug)]
pub struct Ticket {
    task: Option<JoinHandle<Result<Receipt, Error>>>,
    job: Arc<Job>,
}
impl Ticket {
    pub fn cancel(&self) {
        self.job.cancelled.store(true, Ordering::Release);
    }
    pub async fn wait(mut self) -> Result<Receipt, Error> {
        let task = self
            .task
            .take()
            .ok_or_else(|| Error::Unavailable("missing save task".into()))?;
        let result = if let Some(deadline) = self.job.deadline {
            tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), task)
                .await
                .map_err(|_| Error::OutcomeUnknown)?
        } else {
            task.await
        };
        let result = result.map_err(|e| Error::Unavailable(e.to_string()))?;
        if !self.job.allowed() {
            return Err(Error::OutcomeUnknown);
        }
        result
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl SaveWorker {
    pub fn new(path: PathBuf) -> Self {
        let mut file = FallbackFile::new(path);
        Self::with_sink(move |id, document, allowed| file.save_checked(id, document, allowed))
    }
    fn with_sink(
        sink: impl FnMut(Id, &str, &dyn Fn() -> bool) -> Result<(), FileError> + Send + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                sink: Mutex::new(Box::new(sink)),
                slots: Arc::new(Semaphore::new(1)),
                closed: AtomicBool::new(false),
                active: Mutex::new(None),
            }),
        }
    }
    pub fn submit(
        &self,
        save_id: Id,
        document: Arc<str>,
        deadline: Option<Instant>,
    ) -> Result<Ticket, Error> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        if document.len() > dmx_protocol::MAX_RECOVERY_BYTES {
            return Err(Error::Document(ProtocolError::Invalid(
                "fallback snapshot exceeds byte limit".into(),
            )));
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(Error::File(FileError::Interrupted { uncertain: false }));
        }
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|e| Error::Unavailable(e.to_string()))?;
        let permit = self
            .inner
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        let job = Arc::new(Job {
            cancelled: AtomicBool::new(false),
            started: Instant::now(),
            deadline,
        });
        {
            let mut active = self.inner.active.lock().unwrap_or_else(|p| p.into_inner());
            if self.inner.closed.load(Ordering::Acquire) {
                return Err(Error::Closed);
            }
            *active = Some(job.clone());
        }
        let active = Active {
            inner: self.inner.clone(),
            _permit: permit,
        };
        let working = job.clone();
        let span = tracing::Span::current();
        let task=runtime.spawn_blocking(move ||{
            let _active=active;let _span=span.enter();
            let allowed=||working.allowed();
            if !allowed(){return Err(Error::File(FileError::Interrupted{uncertain:false}));}
            let validated=Snapshot::decode(document.as_bytes())?;
            let revision=validated.revision;let digest=validated.digest.clone();drop(validated);
            let mut sink=_active.inner.sink.lock().map_err(|_|Error::Unavailable("fallback sink panicked; outcome unknown".into()))?;
            if !allowed(){return Err(Error::File(FileError::Interrupted{uncertain:false}));}
            tracing::info!(save_id=%String::from(save_id.clone()),snapshot_revision=revision.0,bytes=document.len(),"Saving startup fallback");
            sink(save_id.clone(),&document,&allowed)?;
            if !allowed(){return Err(Error::OutcomeUnknown);}
            tracing::info!(save_id=%String::from(save_id.clone()),snapshot_revision=revision.0,"Startup fallback saved");
            Ok(Receipt{save_id,revision,digest})
        });
        Ok(Ticket {
            task: Some(task),
            job,
        })
    }
    pub fn busy(&self) -> bool {
        self.inner.slots.available_permits() == 0
    }
    pub fn overdue(&self, now: Instant, limit: Duration) -> bool {
        self.inner
            .active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|job| now.saturating_duration_since(job.started) >= limit)
    }
    pub fn close(&self) {
        self.inner.closed.store(true, Ordering::Release);
        if let Some(job) = self
            .inner
            .active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            job.cancelled.store(true, Ordering::Release);
        }
    }
    pub async fn shutdown_until(&self, deadline: Instant) -> bool {
        self.close();
        tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            self.inner.slots.acquire(),
        )
        .await
        .is_ok()
    }
}
impl Drop for SaveWorker {
    fn drop(&mut self) {
        self.close();
    }
}
struct Active {
    inner: Arc<Inner>,
    _permit: OwnedSemaphorePermit,
}
impl Drop for Active {
    fn drop(&mut self) {
        self.inner
            .active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installation::Installation;
    use dmx_protocol::configuration::{Configuration, Content};
    use std::collections::BTreeMap;
    fn id(n: u8) -> Id {
        format!("00000000-0000-0000-0000-{n:012}")
            .try_into()
            .unwrap()
    }
    fn document() -> Arc<str> {
        let content = Content::default();
        let digest = content.digest().unwrap();
        Installation::initialize(
            Configuration {
                schema: 1,
                history: id(1),
                authority: id(2),
                revision: Revision(1),
                expected_previous: None,
                content,
                digest,
            },
            BTreeMap::new(),
        )
        .unwrap()
        .document()
    }
    #[tokio::test]
    async fn only_explicit_validated_submission_writes_and_roundtrips_the_captured_document() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("startup/fallback.json");
        let worker = SaveWorker::new(path.clone());
        assert!(!path.exists());
        let document = document();
        let receipt = worker
            .submit(id(3), document.clone(), None)
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(receipt.save_id, id(3));
        assert_eq!(receipt.revision, Revision(1));
        assert_eq!(receipt.digest, dmx_protocol::sha256(document.as_bytes()));
        assert_eq!(
            FallbackFile::new(path).load().unwrap().unwrap().snapshot,
            document.as_ref()
        );
        assert!(
            worker
                .shutdown_until(Instant::now() + Duration::from_secs(1))
                .await
        );
    }
    #[tokio::test]
    async fn invalid_recovery_never_reaches_the_filesystem_and_worker_remains_usable() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fallback.json");
        let worker = SaveWorker::new(path.clone());
        assert!(matches!(
            worker
                .submit(id(3), Arc::from("{}"), None)
                .unwrap()
                .wait()
                .await,
            Err(Error::Document(_))
        ));
        assert!(!path.exists());
        worker
            .submit(id(4), document(), None)
            .unwrap()
            .wait()
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn cancellation_keeps_one_busy_slot_until_blocking_work_actually_ends() {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let mut entered_tx = Some(entered_tx);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let worker = SaveWorker::with_sink(move |_, _, allowed| {
            let _ = entered_tx.take().unwrap().send(());
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
            if allowed() {
                Ok(())
            } else {
                Err(FileError::Interrupted { uncertain: true })
            }
        });
        let ticket = worker.submit(id(3), document(), None).unwrap();
        entered_rx.await.unwrap();
        ticket.cancel();
        assert!(worker.busy());
        assert!(matches!(
            worker.submit(id(4), document(), None),
            Err(Error::Busy)
        ));
        assert!(worker.overdue(
            Instant::now() + Duration::from_secs(3),
            Duration::from_secs(2)
        ));
        release_tx.send(()).unwrap();
        assert!(matches!(ticket.wait().await, Err(Error::OutcomeUnknown)));
        assert!(!worker.busy());
    }
    #[tokio::test]
    async fn deadline_returns_without_waiting_for_stuck_io_and_shutdown_reports_its_bound() {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let mut entered_tx = Some(entered_tx);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let worker = SaveWorker::with_sink(move |_, _, _| {
            let _ = entered_tx.take().unwrap().send(());
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
            Ok(())
        });
        let ticket = worker
            .submit(
                id(3),
                document(),
                Some(Instant::now() + Duration::from_millis(100)),
            )
            .unwrap();
        entered_rx.await.unwrap();
        assert!(matches!(ticket.wait().await, Err(Error::OutcomeUnknown)));
        assert!(worker.busy());
        assert!(
            !worker
                .shutdown_until(Instant::now() + Duration::from_millis(1))
                .await
        );
        release_tx.send(()).unwrap();
        assert!(
            worker
                .shutdown_until(Instant::now() + Duration::from_secs(1))
                .await
        );
        assert!(matches!(
            worker.submit(id(4), document(), None),
            Err(Error::Closed)
        ));
    }
    #[tokio::test]
    async fn dropping_the_ticket_cancels_before_an_abandoned_job_can_report_success() {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let mut entered_tx = Some(entered_tx);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let observed = cancelled.clone();
        let worker = SaveWorker::with_sink(move |_, _, allowed| {
            let _ = entered_tx.take().unwrap().send(());
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
            observed.store(!allowed(), Ordering::Release);
            Ok(())
        });
        let ticket = worker.submit(id(3), document(), None).unwrap();
        entered_rx.await.unwrap();
        drop(ticket);
        release_tx.send(()).unwrap();
        let _idle = tokio::time::timeout(Duration::from_secs(1), worker.inner.slots.acquire())
            .await
            .unwrap()
            .unwrap();
        assert!(cancelled.load(Ordering::Acquire));
    }
}
