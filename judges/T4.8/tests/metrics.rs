// External judge for T4.8: the built-in metrics of spec/plugins.md, through `TradeEngine::metrics`:
// arch → total_area = Σ rooms[].area; struct → max_utilization = max members[].utilization;
// mep → max_dp = max |segments[].dp|; each 0 when its list is empty. Expected values are recomputed here
// from the result JSON: hand-made results and each engine's own `bim solve --trade <t> --json` output
// (files named by T48_ARCH_JSON, T48_STRUCT_JSON, T48_MEP_JSON, written by judges/T4.8/fixtures.py).
use bim_arch::ArchEngine;
use bim_mep::MepEngine;
use bim_plugin::TradeEngine;
use bim_struct::StructEngine;
use serde_json::{json, Value};

fn metric(e: &dyn TradeEngine, result: &Value, key: &str) -> f64 {
    let m = e.metrics(result);
    let hits: Vec<f64> = m.iter().filter(|x| x.key == key).map(|x| x.value).collect();
    assert_eq!(hits.len(), 1, "engine {}: expected exactly one metric `{key}`, got keys {:?}", e.name(),
        m.iter().map(|x| x.key.clone()).collect::<Vec<_>>());
    hits[0]
}

fn close(got: f64, want: f64, what: &str) {
    assert!(got.is_finite() && (got - want).abs() <= 1e-12 * want.abs().max(1.0), "{what}: got {got}, want {want}");
}

fn field(result: &Value, list: &str, f: &str) -> Vec<f64> {
    result[list].as_array().unwrap_or_else(|| panic!("result has no list `{list}`"))
        .iter().map(|x| x[f].as_f64().unwrap_or_else(|| panic!("{list}[].{f} is not a number: {x}"))).collect()
}

fn solved(var: &str) -> Value {
    let path = std::env::var(var).unwrap_or_else(|_| panic!("{var} is not set (run through judges/T4.8.sh)"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

#[test]
fn engines_have_their_names() {
    assert_eq!(ArchEngine::new().name(), "arch");
    assert_eq!(StructEngine::new().name(), "struct");
    assert_eq!(MepEngine::new().name(), "mep");
}

#[test]
fn arch_total_area() {
    let e = ArchEngine::new();
    let r = json!({"rooms": [{"id": "L1/1", "level": "L1", "area": 10.64, "volume": 30.0, "boundary": [], "space": null},
                             {"id": "L1/2", "level": "L1", "area": 18.24, "volume": 50.0, "boundary": [], "space": null},
                             {"id": "L2/1", "level": "L2", "area": 0.125, "volume": 0.3, "boundary": [], "space": "S"}],
                   "walls": [], "slabs": [], "stairs": [], "curtain_walls": []});
    close(metric(&e, &r, "total_area"), 10.64 + 18.24 + 0.125, "arch total_area (hand-made result)");
    let empty = json!({"rooms": [], "walls": [], "slabs": [], "stairs": [], "curtain_walls": []});
    close(metric(&e, &empty, "total_area"), 0.0, "arch total_area with no rooms");
    let s = solved("T48_ARCH_JSON");
    let want: f64 = field(&s, "rooms", "area").iter().sum();
    assert!(want > 0.0);
    close(metric(&e, &s, "total_area"), want, "arch total_area of bim solve --trade arch output");
}

#[test]
fn struct_max_utilization() {
    let e = StructEngine::new();
    let m = |id: &str, u: f64| json!({"id": id, "N": [0, 0], "Vy": [0, 0], "Vz": [0, 0], "T": [0, 0], "My": [0, 0],
                                      "Mz": [0, 0], "deflection": 0.0, "utilization": u});
    let r = json!({"nodes": [], "members": [m("C1", 0.31), m("B1#1", 0.92), m("B1#2", 0.5)], "reactions": [], "tributary": []});
    close(metric(&e, &r, "max_utilization"), 0.92, "struct max_utilization (hand-made result)");
    let empty = json!({"nodes": [], "members": [], "reactions": [], "tributary": []});
    close(metric(&e, &empty, "max_utilization"), 0.0, "struct max_utilization with no members");
    let s = solved("T48_STRUCT_JSON");
    let want = field(&s, "members", "utilization").into_iter().fold(f64::NEG_INFINITY, f64::max);
    assert!(want > 0.0);
    close(metric(&e, &s, "max_utilization"), want, "struct max_utilization of bim solve --trade struct output");
}

#[test]
fn mep_max_dp() {
    let e = MepEngine::new();
    let g = |id: &str, dp: f64| json!({"id": id, "flow": 0.001, "velocity": 1.0, "dp": dp, "reynolds": 5e4,
                                       "friction_factor": 0.02, "size": "DN25"});
    let r = json!({"segments": [g("S1", 120.0), g("S2", -340.5), g("S3", 20.0)], "ports": [],
                   "continuity_residual": 0.0, "circuits": []});
    close(metric(&e, &r, "max_dp"), 340.5, "mep max_dp = max |dp| (a negative dp is the largest)");
    let empty = json!({"segments": [], "ports": [], "continuity_residual": 0.0, "circuits": []});
    close(metric(&e, &empty, "max_dp"), 0.0, "mep max_dp with no segments");
    let s = solved("T48_MEP_JSON");
    let dps = field(&s, "segments", "dp");
    assert!(dps.iter().any(|d| *d < 0.0), "judge fixture: one segment runs against the flow");
    let want = dps.iter().fold(0.0f64, |a, d| a.max(d.abs()));
    close(metric(&e, &s, "max_dp"), want, "mep max_dp of bim solve --trade mep output");
}
