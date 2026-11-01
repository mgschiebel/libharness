use host_tests::{glue::Spawner, scenarios as sc};
use std::sync::Arc;

fn sp() -> Spawner {
    Arc::new(|f| {
        tokio::spawn(f);
    })
}
macro_rules! t {
    ($($n:ident),*) => {$(
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn $n() { sc::$n(sp()).await }
    )*};
}
t!(typed_result, malformed_results, steer_follow_up, parallel_sessions, abort_running,
   spawn_depth_limit, spawn_fanout_limit, cancel_reaches_spawned_child, cancel_inline_child);
