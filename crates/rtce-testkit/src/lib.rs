//! Golden-fixture harness for rtce and its consumer games.
//!
//! House rules (inherited from diablo4-calc M1): a fixture directory that
//! yields ZERO fixtures is a test failure — an empty glob must never pass
//! silently; every fixture carries `name` and `source` provenance.

use std::path::Path;

/// Relative-tolerance assertion with a context message.
pub fn assert_close(actual: f64, expected: f64, rel_tol: f64, ctx: &str) {
    let denom = expected.abs().max(1e-12);
    let rel = (actual - expected).abs() / denom;
    assert!(
        rel <= rel_tol,
        "{ctx}: {actual} != {expected} (rel err {rel:.3e} > {rel_tol:.1e})"
    );
}

/// Invoke `f(name, json)` for every `*.json` fixture in `dir`, sorted by
/// file name. PANICS if the directory holds no fixtures (or is missing) —
/// the empty-glob rule.
pub fn for_each_fixture(dir: &Path, mut f: impl FnMut(&str, &serde_json::Value)) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("fixture dir {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    entries.sort();
    assert!(
        !entries.is_empty(),
        "no fixtures in {} — an empty suite must not pass",
        dir.display()
    );
    for path in entries {
        let raw =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let v: serde_json::Value = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{}: invalid JSON: {e}", path.display()));
        assert!(
            v.get("name").is_some() && v.get("source").is_some(),
            "{}: fixtures must carry `name` and `source` provenance",
            path.display()
        );
        f(path.file_stem().unwrap().to_str().unwrap(), &v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn assert_close_accepts_within_and_rejects_outside_tolerance() {
        assert_close(100.0005, 100.0, 1e-5, "ok");
        let r = std::panic::catch_unwind(|| assert_close(101.0, 100.0, 1e-5, "off"));
        assert!(r.is_err(), "1% off must fail a 1e-5 tolerance");
    }

    #[test]
    fn empty_fixture_dir_panics() {
        let dir = std::env::temp_dir().join(format!("rtce-testkit-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let r = std::panic::catch_unwind(|| for_each_fixture(&dir, |_, _| {}));
        assert!(r.is_err(), "empty dir must panic");
    }

    #[test]
    fn fixtures_iterate_sorted_and_demand_provenance() {
        let dir = std::env::temp_dir().join(format!("rtce-testkit-two-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("b.json"), r#"{"name":"b","source":"t","v":2}"#).unwrap();
        fs::write(dir.join("a.json"), r#"{"name":"a","source":"t","v":1}"#).unwrap();
        let mut seen = Vec::new();
        for_each_fixture(&dir, |name, v| {
            seen.push((name.to_string(), v["v"].as_i64().unwrap()))
        });
        assert_eq!(seen, vec![("a".into(), 1), ("b".into(), 2)]);

        fs::write(dir.join("c.json"), r#"{"v":3}"#).unwrap();
        let r = std::panic::catch_unwind(|| for_each_fixture(&dir, |_, _| {}));
        assert!(r.is_err(), "missing provenance must panic");
    }
}
