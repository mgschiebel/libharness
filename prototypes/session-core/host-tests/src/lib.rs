//! Host-side glue shared by both executors: scripted fake models, a few tools, the
//! spawn capability, and the scenarios. Nothing here names an executor; the host
//! passes a `Spawner`.
pub mod glue;
pub mod scenarios;
