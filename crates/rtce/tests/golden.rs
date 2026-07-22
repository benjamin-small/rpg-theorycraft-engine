//! Golden fixtures through the full pipeline: JSON → compile → eval →
//! assert. The first fixture is the cross-repo handshake with
//! diablo4-calc's parity suite (base_hit 8,573.0184).

use rtce::expr::compile;
use rtce_testkit::{assert_close, for_each_fixture};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[test]
fn golden_fixtures_reproduce_pinned_values() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for_each_fixture(&dir, |name, v| {
        let slots_json = v["slots"]
            .as_object()
            .unwrap_or_else(|| panic!("{name}: slots"));
        let mut names: Vec<&String> = slots_json.keys().collect();
        names.sort();
        let syms: BTreeMap<String, u16> = names
            .iter()
            .enumerate()
            .map(|(i, n)| ((*n).clone(), i as u16))
            .collect();
        let slots: Vec<f64> = names
            .iter()
            .map(|n| slots_json[*n].as_f64().unwrap())
            .collect();
        let program =
            compile(v["expr"].as_str().unwrap(), &syms).unwrap_or_else(|e| panic!("{name}: {e}"));
        let actual = program.eval(&slots);
        assert_close(
            actual,
            v["expect"].as_f64().unwrap(),
            v["rel_tolerance"].as_f64().unwrap_or(1e-9),
            name,
        );
    });
}
