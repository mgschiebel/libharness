//! What crosses the boundary per call, as serialised JSON bytes. Run with `--nocapture`
//! to see the table. Asserts only loose shape facts; the numbers are the measurement.
use call_core::*;
use serde_json::{json, Value};

fn spec() -> Spec {
    let t = |n: &str, d: &str| ToolSpec { name: n.into(), description: d.into(), params: json!({"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}}}) };
    Spec {
        tools: vec![t("add", "add two ints"), t("echo", "echo the input")],
        result_schema: json!({"type":"object","properties":{"score":{"type":"integer"},"label":{"type":"string"}},"required":["score","label"]}),
    }
}
fn validate(v: &Value) -> Result<(), String> {
    (v["score"].is_u64() && v["label"].is_string()).then_some(()).ok_or_else(|| "bad".to_string())
}
fn b<T: serde::Serialize>(t: &T) -> usize {
    serde_json::to_string(t).unwrap().len()
}

#[test]
fn bytes_per_call_for_a_sample_conversation() {
    let sp = spec();
    println!("spec (tools + schema, read-only, not per call state): {} bytes", b(&sp));
    println!("{:>4} {:<28} {:>11} {:>14} {:>13}", "step", "what", "state bytes", "request bytes", "msgs in state");
    let mut o = begin(start(SessionId(1), Config::default(), "Add 2 and 3, then give me a verdict."), &sp);
    let mut step = 0;
    let mut rows = vec![];
    let row = |what: &str, o: &Out, rows: &mut Vec<(usize, usize)>| {
        let req = if let Step::Call(r) = &o.step { b(r) } else { 0 };
        println!("{:>4} {:<28} {:>11} {:>14} {:>13}", rows.len(), what, b(&o.state), req, o.state.messages.len());
        rows.push((b(&o.state), req));
    };
    row("begin (prompt only)", &o, &mut rows);
    for i in 0..5 {
        let id = format!("c{i}");
        let reply = ModelReply { text: "calling add".into(), tool_calls: vec![ToolCall { id: id.clone(), name: "add".into(), args: json!({"a": i, "b": i + 1}) }], tokens: 12 };
        o = absorb(o.state, &sp, &validate, Ok(reply));
        row(&format!("after model reply {i} (tools)"), &o, &mut rows);
        o = tools_done(o.state, &sp, vec![ToolDone { call_id: id, result: Ok(json!(2 * i + 1)) }]);
        row(&format!("after tool results {i}"), &o, &mut rows);
        step += 1;
    }
    let reply = ModelReply { text: String::new(), tool_calls: vec![ToolCall { id: "fa".into(), name: "final_answer".into(), args: json!({"result": {"score": 7, "label": "ok"}}) }], tokens: 20 };
    o = absorb(o.state, &sp, &validate, Ok(reply));
    row("after final_answer (End)", &o, &mut rows);
    assert!(matches!(o.step, Step::End(Ok(_))) && step == 5);
    // linear growth per turn, not per-call overhead
    let per_turn = (rows[10].0 - rows[0].0) / 5;
    println!("state growth per tool turn (assistant msg + tool result): about {per_turn} bytes");
    println!("sum of request bytes over the run (what the model callback is handed in total): {}", rows.iter().map(|r| r.1).sum::<usize>());
    assert!(rows[10].0 > rows[0].0);
    // empty-state floor
    let fresh = start(SessionId(1), Config::default(), "");
    println!("empty state (no prompt text): {} bytes; of which config {} bytes", b(&fresh), b(&fresh.cfg));
}
