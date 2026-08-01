//! `rtce-testkit` — golden-fixture harness for `rtce` and its consumer
//! games.
//!
//! This crate carries no test logic of its own; it's the small set of
//! house rules every `rtce`-based game's test suite is expected to share,
//! inherited from `d4-theory-crafting`'s M1 milestone:
//!
//! - **Golden fixtures live in JSON files, one case per file.**
//!   [`for_each_fixture`] walks a directory, parses each `*.json` as a
//!   `serde_json::Value`, and hands it to your callback in sorted-by-name
//!   order (deterministic test output, deterministic diffs).
//! - **An empty suite is a bug, not a pass.** If the fixture directory is
//!   missing or yields zero `*.json` files, [`for_each_fixture`] panics
//!   instead of silently iterating zero times — a typo'd path or an
//!   accidentally-emptied directory must fail loudly, never look like
//!   "all fixtures passed."
//! - **Every fixture is provenance-tagged.** Each JSON file must carry a
//!   `name` and a `source` key (where the expected number came from — a
//!   hand-worked calculation, an in-game screenshot, another calculator)
//!   so a reviewer can trace any pinned number back to its origin.
//! - **Comparisons are relative-tolerance, not exact-float.** [`assert_close`]
//!   compares `actual` against `expected` within a relative tolerance,
//!   because floating-point pipelines rarely reproduce bit-for-bit across
//!   platforms/optimization levels.
//!
//! Depend on this crate as a dev-dependency; use [`for_each_fixture`] to
//! drive your golden-fixture tests and [`assert_close`] to check each
//! result.

#![warn(missing_docs)]

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
