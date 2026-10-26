//! Adapt the adopter's existing native observation sites to bounded snapshots.

use metewand_runtime::{
    buffered::{BufferedTrace, Decision, Progress},
    worker_sdk::{WorkerError, WorkerResult},
};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::BTreeMap};

struct Observer {
    trace: BufferedTrace,
    next: u64,
    stopped: bool,
    error: Option<WorkerError>,
    last: Option<(usize, Vec<f64>, Progress, Value)>,
}

thread_local! {
    static OBSERVER: RefCell<Option<Observer>> = const { RefCell::new(None) };
}

pub(crate) fn stopped() -> bool {
    OBSERVER.with_borrow(|o| o.as_ref().is_some_and(|o| o.stopped))
}

pub(crate) fn record(iteration: usize, beta: &[f64], epsilon_g: f64, work: [usize; 8]) -> bool {
    OBSERVER.with_borrow_mut(|observer| {
        let Some(observer) = observer.as_mut() else {
            return false;
        };
        if observer.stopped
            || observer
                .last
                .as_ref()
                .is_some_and(|(i, b, _, _)| *i == iteration && b == beta)
        {
            return true;
        }
        let progress = Progress {
            check_index: observer.next,
            work: crate::trajectory::WORK_NAMES
                .iter()
                .zip(work)
                .map(|(k, v)| (k.to_string(), v as u64))
                .collect::<BTreeMap<_, _>>(),
        };
        let metrics = json!({"epsilon_g": epsilon_g});
        let decision = observer
            .trace
            .record(progress.clone(), metrics.clone(), || {
                Ok(json!({"coefficients": beta, "intercept": 0.0}))
            });
        observer.last = Some((iteration, beta.to_vec(), progress, metrics));
        observer.next += 1;
        match decision {
            Ok(Decision::Continue) => {}
            Ok(Decision::Stop(_)) => observer.stopped = true,
            Err(error) => {
                observer.error = Some(error);
                observer.stopped = true;
            }
        }
        true
    })
}

pub(crate) fn collect(
    trace: BufferedTrace,
    solve: impl FnOnce() -> WorkerResult<Value>,
) -> WorkerResult<BufferedTrace> {
    OBSERVER.with_borrow_mut(|o| {
        *o = Some(Observer {
            trace,
            next: 0,
            stopped: false,
            error: None,
            last: None,
        })
    });
    let result = solve();
    let mut observer = OBSERVER
        .with_borrow_mut(Option::take)
        .expect("installed observer");
    result?;
    if let Some(error) = observer.error {
        return Err(error);
    }
    let (_, beta, progress, metrics) = observer
        .last
        .ok_or_else(|| WorkerError::operation_failed("native solver returned no checks"))?;
    observer.trace.finish(
        progress,
        || Ok(metrics),
        || Ok(json!({"coefficients": beta, "intercept": 0.0})),
    )?;
    Ok(observer.trace)
}
