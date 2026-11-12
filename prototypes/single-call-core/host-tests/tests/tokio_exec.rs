use host_tests::{glue::*, scenarios as sc};

fn sp(o: Owner) -> Spawner {
    Spawner::new(o, |f| {
        tokio::spawn(f);
    })
}
macro_rules! t {
    ($m:ident, $o:expr; $($n:ident),*) => {
        mod $m {
            use super::*;
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
                async fn $n() { sc::$n(sp($o)).await }
            )*
        }
    };
}
t!(shipped, Owner::Shipped; typed_result, malformed_results, steer_follow_up, parallel_sessions, abort_running,
   spawn_depth_limit, spawn_fanout_limit, cancel_reaches_spawned_child, cancel_inline_child);
t!(injected, Owner::Injected; typed_result, malformed_results, steer_follow_up, parallel_sessions, abort_running,
   spawn_depth_limit, spawn_fanout_limit, cancel_reaches_spawned_child, cancel_inline_child, checkpoint_resume);
