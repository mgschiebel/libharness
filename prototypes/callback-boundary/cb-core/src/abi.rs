//! The job a host gives a Wasm guest, and what comes back. Plain serialisable data.
use crate::link::*;
use alloc::string::String;
use call_core::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
pub enum Job {
    /// `spec: None` means "use the registrations this instance already holds" (R2).
    Loop { state: State, spec: Option<Spec> },
    Exchange { req: HttpReq },
}
#[derive(Serialize, Deserialize)]
pub struct GuestIn {
    /// true: one import per operation kind; false: one generic `call(name, args)` import.
    pub typed_imports: bool,
    pub job: Job,
}
#[derive(Serialize, Deserialize)]
pub enum GuestOut {
    Loop { state: State, result: Result<Value, SessionError> },
    Exchange(Option<Result<ModelReply, String>>),
}

/// Run a job over any `Link`. The guest calls this; so can any native host.
pub async fn run_job(link: &dyn Link, job: Job, held: Option<Spec>) -> GuestOut {
    match job {
        Job::Loop { state, spec } => {
            let spec = spec.or(held).expect("no spec: register one or pass one");
            let (state, result) = drive(link, &spec, state).await;
            GuestOut::Loop { state, result }
        }
        Job::Exchange { req } => GuestOut::Exchange(exchange(link, req).await),
    }
}
