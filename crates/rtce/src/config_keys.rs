//! Fail-closed unknown-key rejection for every config struct (P8a), with
//! one carve-out: keys starting with `_` are the documented ANNOTATION
//! NAMESPACE (`_source`, `_scope`, `_shape`, …) and are accepted at every
//! nesting level — the committed fixtures and both consumers' gamedefs
//! carry them, which is exactly why a blanket serde-level
//! `deny_unknown_fields` was rejected in the P8 design.
//!
//! Two mechanisms share [`reject_unknown`], differing only in WHERE the
//! walk runs:
//!
//! - Structs whose identity lives OUTSIDE them — as the key of the
//!   registry map they sit in (`buffs: { "poison": … }`) — store the
//!   collected keys in a public `#[serde(flatten)] extra` field, and
//!   `plan::compile` / `sim::compile` walk those maps where the registry
//!   name is in hand, so the error can say "on buff `poison`". These are
//!   the `SimDef`-side structs plus [`crate::gamedef::EventDef`]. The
//!   `_`-prefixed keys survive serde round-trips through that field.
//! - Structs both consumers construct in Rust with EXHAUSTIVE struct
//!   literals ([`crate::gamedef::GameDef`]/`BucketDef`/`StageDef`,
//!   [`crate::build::BuildState`]/`Contribution`,
//!   [`crate::scenario::Scenario`]/`Phase`) cannot grow a field without
//!   breaking those consumers, so they use the spec's "hand-written
//!   equivalent": a manual `Deserialize` collects leftovers into a
//!   parse-local map and calls [`reject_unknown`] right there, with the
//!   context the struct itself carries (`phase `boss``). Annotations are
//!   accepted and DROPPED on parse — the same fate they had under the
//!   0.3.0 derived `Deserialize`, so nothing regresses.
//!
//! A new config struct (e.g. P8c's `defaults` block) should take the
//! FIRST shape unless a consumer already constructs it in Rust.
//!
//! # Duplicate keys
//!
//! Stated for the record (and pinned in this module's tests), since
//! `#[serde(flatten)]` changes serde's usual story only for UNKNOWN
//! keys:
//!
//! - A duplicate KNOWN field is still a serde error ("duplicate field
//!   `duration`"), on both mechanisms — the flatten-bearing structs and
//!   the hand-written mirrors alike.
//! - A duplicate UNKNOWN key resolves LAST-WINS (the collection map is a
//!   `BTreeMap`; the second insert replaces the first). Since every
//!   non-`_` unknown key is rejected anyway, the only OBSERVABLE
//!   last-wins case is a duplicate `_` annotation — a config carrying
//!   `"_note"` twice keeps the second, silently. Accepted as harmless:
//!   annotations carry no semantics.

use crate::plan::PlanError;
use std::collections::BTreeMap;

/// Fail closed on the first (lexicographically) non-`_` key in `extra`:
/// the error names the key, the `context` it sits on ("buff `poison`"),
/// and the nearest of the `known` field names by edit distance (≤ 2) as a
/// did-you-mean — or lists every known field when nothing is close.
/// `_`-prefixed keys are the annotation namespace and pass untouched.
pub(crate) fn reject_unknown(
    context: &str,
    known: &[&str],
    extra: &BTreeMap<String, serde_json::Value>,
) -> Result<(), PlanError> {
    for key in extra.keys() {
        if key.starts_with('_') {
            continue;
        }
        let what = match nearest(key, known) {
            Some(best) => {
                format!("unknown field `{key}` on {context} — did you mean `{best}`?")
            }
            None => format!(
                "unknown field `{key}` on {context}; expected one of: {}",
                known
                    .iter()
                    .map(|k| format!("`{k}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        return Err(PlanError { what });
    }
    Ok(())
}

/// The `known` name nearest to `key` by [`edit_distance`], if any is
/// within distance 2 (a typo, not a different word). Ties keep the
/// earlier entry in `known` — deterministic, and `known` lists are
/// declaration-ordered.
fn nearest<'a>(key: &str, known: &[&'a str]) -> Option<&'a str> {
    let mut best: Option<(&str, usize)> = None;
    for k in known {
        let d = edit_distance(key, k);
        if d <= 2 && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((k, d));
        }
    }
    best.map(|(k, _)| k)
}

/// Plain Levenshtein distance (insert/delete/substitute, all cost 1), the
/// standard two-row DP — hand-rolled because the crate's zero-dependency
/// rule is absolute. A transposition therefore costs 2, which still lands
/// inside the ≤ 2 suggestion window (`idc` → `icd`).
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extra_of(keys: &[&str]) -> BTreeMap<String, serde_json::Value> {
        keys.iter()
            .map(|k| ((*k).to_string(), serde_json::Value::Null))
            .collect()
    }

    #[test]
    fn edit_distance_hand_worked() {
        assert_eq!(edit_distance("when", "when"), 0);
        assert_eq!(edit_distance("whn", "when"), 1); // one insert
        assert_eq!(edit_distance("uptime", "uptimes"), 1); // one insert
        assert_eq!(edit_distance("tick_objectiv", "tick_objective"), 1);
        assert_eq!(edit_distance("idc", "icd"), 2); // transposition = 2 subs
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("stats", "phases"), 4); // beyond the window
    }

    #[test]
    fn underscore_keys_pass_and_everything_else_fails() {
        let known = &["stats", "contributions"];
        assert!(reject_unknown("the build state", known, &extra_of(&["_source"])).is_ok());
        let e = reject_unknown("the build state", known, &extra_of(&["stat"])).unwrap_err();
        assert!(e.what.contains("unknown field `stat`"), "got: {}", e.what);
        assert!(e.what.contains("the build state"), "got: {}", e.what);
        assert!(e.what.contains("did you mean `stats`"), "got: {}", e.what);
    }

    #[test]
    fn a_key_far_from_everything_lists_the_known_fields_instead() {
        let known = &["duration", "conditions"];
        let e = reject_unknown("buff `x`", known, &extra_of(&["banana"])).unwrap_err();
        assert!(
            e.what.contains("expected one of: `duration`, `conditions`"),
            "got: {}",
            e.what
        );
        assert!(!e.what.contains("did you mean"), "got: {}", e.what);
    }

    // The serde half of the "Duplicate keys" module-doc claim, pinned on
    // BOTH mechanisms so it cannot drift: a duplicate KNOWN field still
    // errors with flatten in play (flatten only re-routes UNKNOWN keys).
    #[test]
    fn a_duplicate_known_field_is_still_a_serde_error_on_both_mechanisms() {
        // Flatten-bearing struct:
        let e = serde_json::from_str::<crate::simdef::BuffDef>(
            r#"{ "duration": 1.0, "duration": 2.0 }"#,
        )
        .unwrap_err();
        assert!(
            e.to_string().contains("duplicate field `duration`"),
            "got: {e}"
        );
        // Hand-written mirror:
        let e = serde_json::from_str::<crate::scenario::Phase>(
            r#"{ "name": "p", "weight": 1, "name": "q" }"#,
        )
        .unwrap_err();
        assert!(e.to_string().contains("duplicate field `name`"), "got: {e}");
    }

    #[test]
    fn the_first_offending_key_in_sorted_order_is_reported() {
        // BTreeMap iterates sorted, and `_` sorts before ASCII letters —
        // an annotation never masks a typo, and the reported key is
        // deterministic.
        let known = &["duration"];
        let e =
            reject_unknown("buff `x`", known, &extra_of(&["zzz", "durat", "_note"])).unwrap_err();
        assert!(e.what.contains("`durat`"), "got: {}", e.what);
    }
}
