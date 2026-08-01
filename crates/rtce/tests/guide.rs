//! The `docs/guide/` drift gate.
//!
//! Every chapter of the guide embeds its configuration as fenced JSON,
//! and the chapter's example program `include_str!`s the same
//! configuration from `tests/fixtures/guide/`. Those two copies must not
//! drift, so this suite asserts the prose block is byte-identical to the
//! file the example actually runs.
//!
//! The configs live under `tests/fixtures/` — alongside the `d4`/`poe2`
//! fixtures, and NOT under `docs/` — because `cargo package` only ships
//! files inside the crate root. An example that `include_str!`s out of
//! the crate compiles here and fails for anyone who installs from
//! crates.io, and `cargo publish --dry-run` does not catch it: its
//! verify step builds the library, not the examples.
//!
//! The convention the guide uses:
//!
//! ```text
//! ```json title=04-build.json
//! { … }
//! ```
//! ```
//!
//! `title=` is mandatory. An untitled ```json block would be prose no
//! test could check, so [`every_json_block_is_titled`] rejects one —
//! otherwise the gate could be bypassed simply by leaving the title off.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Where the prose lives.
fn guide_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/guide")
}

/// Where the configs the examples actually run live — inside the crate,
/// so they package. See this module's docs.
fn configs_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/guide")
}

/// Every `docs/guide/*.md`, sorted, so failures name a stable file.
fn chapters() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(guide_dir())
        .expect("docs/guide exists")
        .map(|e| e.expect("readable entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    out.sort();
    out
}

/// One fenced block in a chapter: the `title=` it claimed, its body, and
/// the line the fence opened on (for a diagnosable failure message).
struct Block {
    title: Option<String>,
    body: String,
    line: usize,
}

/// Pull every ```json fence out of one chapter.
///
/// Deliberately dumb: the guide is hand-written prose we control, so a
/// line-scanner is enough and a markdown dependency would not be. Fences
/// are matched at column zero only.
fn json_blocks(markdown: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut lines = markdown.lines().enumerate();

    while let Some((i, line)) = lines.next() {
        let Some(info) = line.strip_prefix("```json") else {
            continue;
        };
        let title = info
            .trim()
            .strip_prefix("title=")
            .map(|t| t.trim().to_string());

        let mut body = String::new();
        for (_, inner) in lines.by_ref() {
            if inner.starts_with("```") {
                break;
            }
            body.push_str(inner);
            body.push('\n');
        }
        blocks.push(Block {
            title,
            body,
            line: i + 1,
        });
    }
    blocks
}

/// The gate: a chapter's fenced JSON is the config file its example runs.
#[test]
fn every_titled_block_matches_its_config_file() {
    let mut checked = 0usize;

    for chapter in chapters() {
        let markdown = std::fs::read_to_string(&chapter).expect("chapter is readable");
        let name = chapter.file_name().expect("named file").to_string_lossy();

        for block in json_blocks(&markdown) {
            let Some(title) = block.title else {
                continue; // reported by `every_json_block_is_titled`
            };
            let config = configs_dir().join(&title);
            let on_disk = std::fs::read_to_string(&config).unwrap_or_else(|e| {
                panic!(
                    "{name}:{} names `{title}`, which does not exist: {e}",
                    block.line
                )
            });

            assert_eq!(
                block.body.trim_end(),
                on_disk.trim_end(),
                "{name}:{} has drifted from tests/fixtures/guide/{title} — \
                 the chapter and the example it links to now disagree",
                block.line
            );
            checked += 1;
        }
    }

    // A gate that silently checks nothing is worse than no gate: if the
    // scanner ever stops recognising the guide's fences, fail loudly
    // rather than pass vacuously.
    assert!(
        checked >= 15,
        "expected the guide to embed at least 15 titled configs, found {checked} — \
         has the fence convention changed?"
    );
}

/// `title=` is not optional. Without this, dropping the title would turn
/// a checked block back into unchecked prose.
#[test]
fn every_json_block_is_titled() {
    for chapter in chapters() {
        let markdown = std::fs::read_to_string(&chapter).expect("chapter is readable");
        let name = chapter.file_name().expect("named file").to_string_lossy();

        for block in json_blocks(&markdown) {
            assert!(
                block.title.is_some(),
                "{name}:{} opens an untitled ```json fence — every JSON block in the \
                 guide must be ```json title=<file>, naming a file in tests/fixtures/guide/",
                block.line
            );
        }
    }
}

/// Every config on disk is reachable: each one is `include_str!`d by at
/// least one guide example. Catches a config left behind by an edit,
/// which would otherwise sit there looking authoritative forever.
#[test]
fn every_config_is_used_by_an_example() {
    let examples_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut referenced = BTreeSet::new();

    for entry in std::fs::read_dir(&examples_dir).expect("examples/ exists") {
        let path = entry.expect("readable entry").path();
        if !path.file_name().is_some_and(|n| {
            n.to_string_lossy().starts_with("guide_") && path.extension().is_some_and(|x| x == "rs")
        }) {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("example is readable");
        const PREFIX: &str = "tests/fixtures/guide/";
        for (at, _) in src.match_indices(PREFIX) {
            let rest = &src[at + PREFIX.len()..];
            // Everything up to the closing quote of the `include_str!`
            // path literal.
            if let Some(end) = rest.find('"') {
                referenced.insert(rest[..end].to_string());
            }
        }
    }

    let mut orphans = Vec::new();
    for entry in std::fs::read_dir(configs_dir()).expect("configs dir exists") {
        let path = entry.expect("readable entry").path();
        let name = path
            .file_name()
            .expect("named file")
            .to_string_lossy()
            .to_string();
        if !referenced.contains(&name) {
            orphans.push(name);
        }
    }
    orphans.sort();

    assert!(
        orphans.is_empty(),
        "tests/fixtures/guide/ contains files no guide example runs: {orphans:?} — \
         either wire them into a chapter's example or delete them"
    );
}

/// No example may `include_str!` its way out of the crate.
///
/// `cargo package` ships only files under the crate root, so an example
/// reading `../../../docs/…` builds from a git checkout and fails for
/// everyone who installs from crates.io. `cargo publish --dry-run` does
/// NOT catch this — its verify step builds the library, not the
/// examples — so the guard has to live here. (The guide's configs were
/// briefly written under `docs/guide/configs/` for exactly this reason,
/// and moved to `tests/fixtures/guide/` when the packaged crate turned
/// out not to compile them.)
#[test]
fn no_example_includes_from_outside_the_crate() {
    let examples_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut escapes = Vec::new();

    for entry in std::fs::read_dir(&examples_dir).expect("examples/ exists") {
        let path = entry.expect("readable entry").path();
        if path.extension().is_none_or(|x| x != "rs") {
            continue;
        }
        let name = path.file_name().expect("named file").to_string_lossy();
        let src = std::fs::read_to_string(&path).expect("example is readable");

        for (at, _) in src.match_indices("include_str!(") {
            let rest = &src[at..];
            let Some(open) = rest.find('"') else { continue };
            let Some(close) = rest[open + 1..].find('"') else {
                continue;
            };
            let literal = &rest[open + 1..open + 1 + close];

            // `examples/` is one level below the crate root, so exactly
            // one leading `../` stays inside it. Two or more escapes.
            if literal.starts_with("../../") {
                escapes.push(format!("{name}: {literal}"));
            }
        }
    }
    escapes.sort();

    assert!(
        escapes.is_empty(),
        "these examples read files from outside the crate, so they will not compile \
         for anyone who installs rtce from crates.io: {escapes:?} — move the file under \
         crates/rtce/ (tests/fixtures/ is where the other fixtures live)"
    );
}
