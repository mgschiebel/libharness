//! Every scenario under every boundary. B1 = closures with handles, B2 = JSON bytes in
//! process, B3 = Wasm guest (typed imports, and the one generic `call` import).
use cb_host::rig::*;
use cb_host::scenarios as sc;
use futures::executor::block_on;

macro_rules! suite {
    ($($name:ident),*) => { $( mod $name {
        use super::*;
        #[test] fn b1() { block_on(sc::$name(Rig::new(Boundary::B1))); }
        #[test] fn b2() { block_on(sc::$name(Rig::new(Boundary::B2))); }
        #[test] fn b3_typed() { block_on(sc::$name(Rig::new(Boundary::B3Typed))); }
        #[test] fn b3_generic() { block_on(sc::$name(Rig::new(Boundary::B3Generic))); }
        #[test] fn r2_snapshot() { block_on(sc::$name(Rig::new(Boundary::B1CtxSnapshot))); }
        #[test] fn r2_live() { block_on(sc::$name(Rig::new(Boundary::B1CtxLive))); }
    } )* };
}
suite!(typed_result, malformed_results, steer_follow_up, spawn_child, parallel_runs);

macro_rules! suite_ret {
    ($($name:ident),*) => { $( mod $name {
        use super::*;
        #[test] fn b1() { println!("{} B1 {:?}", stringify!($name), block_on(sc::$name(Rig::new(Boundary::B1)))); }
        #[test] fn b2() { println!("{} B2 {:?}", stringify!($name), block_on(sc::$name(Rig::new(Boundary::B2)))); }
        #[test] fn b3_typed() { println!("{} B3t {:?}", stringify!($name), block_on(sc::$name(Rig::new(Boundary::B3Typed)))); }
        #[test] fn b3_generic() { println!("{} B3g {:?}", stringify!($name), block_on(sc::$name(Rig::new(Boundary::B3Generic)))); }
        #[test] fn r2_snapshot() { println!("{} R2s {:?}", stringify!($name), block_on(sc::$name(Rig::new(Boundary::B1CtxSnapshot)))); }
        #[test] fn r2_live() { println!("{} R2l {:?}", stringify!($name), block_on(sc::$name(Rig::new(Boundary::B1CtxLive)))); }
    } )* };
}
suite_ret!(abort_hang, cancel_reaches_child, cancel_reaches_spawned_child, stream_deltas, transport_stream, transport_cancel);
