//! Same scenario functions, different executors: futures-executor ThreadPool and
//! the single-threaded LocalPool; owner (shipped / injected loop) is a second axis.
use futures::executor::{block_on, LocalPool, ThreadPool};
use futures::task::{LocalSpawnExt, SpawnExt};
use host_tests::{glue::*, scenarios as sc};

macro_rules! t {
    ($m:ident, $o:expr; $($n:ident),*) => {
        mod $m {
            use super::*;
            $(
                mod $n {
                    use super::*;
                    #[test]
                    fn thread_pool() {
                        let pool = ThreadPool::new().unwrap();
                        let sp = Spawner::new($o, move |f| pool.spawn(f).unwrap());
                        block_on(sc::$n(sp));
                    }
                    #[test]
                    fn local_pool() {
                        let mut pool = LocalPool::new();
                        // LocalSpawner is !Send; capabilities must be Send, so the Spawner handle
                        // forwards through a channel to a task living on the LocalPool.
                        let (tx, mut rx) = futures::channel::mpsc::unbounded::<futures::future::BoxFuture<'static, ()>>();
                        let spawner = pool.spawner();
                        let s2 = spawner.clone();
                        spawner.spawn_local(async move {
                            while let Some(f) = futures::StreamExt::next(&mut rx).await {
                                s2.spawn_local(f).unwrap();
                            }
                        }).unwrap();
                        let sp = Spawner::new($o, move |f| tx.unbounded_send(f).unwrap());
                        pool.run_until(sc::$n(sp));
                    }
                }
            )*
        }
    };
}
t!(shipped, Owner::Shipped; typed_result, malformed_results, steer_follow_up, parallel_sessions, abort_running,
   spawn_depth_limit, spawn_fanout_limit, cancel_reaches_spawned_child, cancel_inline_child);
t!(injected, Owner::Injected; typed_result, malformed_results, steer_follow_up, parallel_sessions, abort_running,
   spawn_depth_limit, spawn_fanout_limit, cancel_reaches_spawned_child, cancel_inline_child, checkpoint_resume);
