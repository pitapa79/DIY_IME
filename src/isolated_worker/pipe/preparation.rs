//! Typed construction ownership. Partial bytes cannot escape a live process.
use super::*;
use crate::isolated_worker::adapter::{
    ArtifactCapture, PreparationIdentity, PreparationLimits, PreparationSources, PreparedArtifact,
    ResourceRangeSession,
};
use crate::node::CoreError;
use std::sync::Mutex;

pub struct PreparationWorkerRequest {
    pub input: Vec<u8>,
    pub sources: PreparationSources,
    pub identity: PreparationIdentity,
    pub limits: PreparationLimits,
}
pub enum PreparationOutcome<'a> {
    CancelledBeforeStart,
    Exited(PreparationExit),
    Unconfirmed(PreparationRunning<'a>),
}
pub struct PreparationExit {
    receipt: ExitReceipt,
    artifact: Result<Box<PreparedArtifact>, CoreError>,
}
impl PreparationExit {
    pub fn receipt(&self) -> &ExitReceipt {
        &self.receipt
    }
    pub fn artifact(&self) -> Result<&PreparedArtifact, &CoreError> {
        self.artifact.as_deref()
    }
    pub fn into_artifact(self) -> Result<PreparedArtifact, CoreError> {
        self.artifact.map(|artifact| *artifact)
    }
}
/// Field order also keeps source/output reservations alive while Running::drop
/// stops and joins the original Worker. No guest ending claim releases them.
pub struct PreparationRunning<'a> {
    running: Box<Running<'a>>,
    ranges: Arc<Mutex<ResourceRangeSession>>,
    capture: Arc<Mutex<ArtifactCapture>>,
    identity: PreparationIdentity,
    source_sha256: String,
}
impl<'a> PreparationRunning<'a> {
    pub fn process_id(&self) -> u32 {
        self.running.process_id()
    }
    pub fn retry(self, grace: Duration) -> PreparationOutcome<'a> {
        let Self {
            running,
            ranges,
            capture,
            identity,
            source_sha256,
        } = self;
        settle(
            running.retry(grace),
            ranges,
            capture,
            identity,
            source_sha256,
        )
    }
}

fn settle<'a>(
    outcome: Outcome<'a>,
    ranges: Arc<Mutex<ResourceRangeSession>>,
    capture: Arc<Mutex<ArtifactCapture>>,
    identity: PreparationIdentity,
    source_sha256: String,
) -> PreparationOutcome<'a> {
    match outcome {
        Outcome::CancelledBeforeStart => PreparationOutcome::CancelledBeforeStart,
        Outcome::Unconfirmed(running) => PreparationOutcome::Unconfirmed(PreparationRunning {
            running,
            ranges,
            capture,
            identity,
            source_sha256,
        }),
        Outcome::Exited(receipt) => {
            // The original process has ended and its IO thread was joined.
            let mut ranges = ranges.lock().unwrap_or_else(|p| p.into_inner());
            let mut capture = capture.lock().unwrap_or_else(|p| p.into_inner());
            let artifact = capture
                .settle(&receipt, &mut ranges, identity, source_sha256)
                .map(Box::new);
            if artifact.is_err() {
                ranges.withdraw();
            }
            PreparationOutcome::Exited(PreparationExit { receipt, artifact })
        }
    }
}

/// Native composition must pin the actual verified code/config to identity,
/// and select the construction-only Worker arguments. No package opt-in here.
pub fn execute_preparation<'a>(
    profile: &'a Profile,
    job: &'a Job,
    cache: &'a Cache,
    program: &Path,
    args: &[OsString],
    mut request: PreparationWorkerRequest,
    control: &RunControl,
) -> Result<PreparationOutcome<'a>, String> {
    if request.input.len() > super::WIRE_LIMIT
        || !request.sources.ranges.cancellation_matches(&control.cancel)
    {
        request.sources.ranges.withdraw();
        return Err("invalid preparation invocation owner or input bound".into());
    }
    request
        .identity
        .validate()
        .map_err(|_| "invalid preparation identity")?;
    request
        .sources
        .ranges
        .check_current()
        .map_err(|_| "preparation unavailable")?;
    let mut control = control.clone();
    let remaining = request.sources.ranges.remaining_budget();
    let resource_deadline = Some(request.sources.ranges.absolute_deadline());
    control.walltime = Some(control.walltime.map_or(remaining, |n| n.min(remaining)));
    let capture = Arc::new(Mutex::new(
        ArtifactCapture::new(request.limits).map_err(|_| "artifact reservation unavailable")?,
    ));
    let ranges = Arc::new(Mutex::new(request.sources.ranges));
    let io_ranges = ranges.clone();
    let io_capture = capture.clone();
    let exchange = Box::new(move |output, input| {
        let mut ranges = io_ranges
            .lock()
            .map_err(|_| "preparation owner unavailable")?;
        let mut capture = io_capture
            .lock()
            .map_err(|_| "preparation capture unavailable")?;
        let result = crate::isolated_worker::resource_transport::serve_build(
            output,
            input,
            &request.input,
            &mut ranges,
            &mut capture,
        );
        if result.is_err() {
            ranges.withdraw();
        }
        result.map_err(|_| "Worker preparation exchange failed".into())
    });
    let outcome = execute_observed(
        WorkerScope {
            profile,
            job,
            cache,
            activity: None,
            resource_deadline,
            exchange: Some(exchange),
            range_session: Some(ranges.clone()),
            #[cfg(test)]
            prepared: false,
        },
        program,
        args,
        Vec::new(),
        &control,
    )?;
    Ok(settle(
        outcome,
        ranges,
        capture,
        request.identity,
        request.sources.source_sha256,
    ))
}
