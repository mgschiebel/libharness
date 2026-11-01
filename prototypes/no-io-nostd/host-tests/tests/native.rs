use nostd_core::{demo, VecStream};
use futures::StreamExt;
#[test]
fn same_core_runs_natively() {
    let caps = demo::caps();
    let a = futures::executor::block_on(caps.run_once("hi")).unwrap();
    assert_eq!(a, demo::Answer { total: 5, note: "done".into() });
    assert_eq!(caps.calls_made(), 1);
    assert_eq!(caps.now_ms(), 42);
    assert_eq!(caps.tool_specs()[0].name, "add");
    assert!(caps.result_schema().is_object());
    let v: Vec<i32> = futures::executor::block_on(VecStream(vec![1, 2, 3]).collect());
    assert_eq!(v, [1, 2, 3]);
}
