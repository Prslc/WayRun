use std::path::Path;

use crate::provider::file::index::build::build;
use crate::provider::file::index::format::{Index, layout_ok};
use crate::wire::ResultItem;

pub(super) fn write(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, "x").unwrap();
}

pub(super) fn parsed<'a>(bytes: &'a [u8], home: &Path) -> Index<'a> {
    let index = Index::parse(bytes, home).expect("a freshly built index parses");
    assert!(layout_ok(&index), "a freshly built index keeps the layout");
    index
}

/// The names a stock install skips: the shipped template's list, the one
/// place those defaults are written down.
pub(super) fn shipped_exclude() -> Vec<String> {
    toml::from_str::<crate::config::Config>(crate::config::DEFAULT_TEMPLATE)
        .expect("the template parses")
        .files
        .exclude
}

/// A stock install's behaviour: the shipped `[files] exclude` list.
pub(super) fn build_ok(home: &Path, cap: usize) -> Vec<u8> {
    build(home, cap, &shipped_exclude()).expect("a readable home builds")
}

pub(super) fn summaries(items: &[ResultItem]) -> Vec<String> {
    items
        .iter()
        .map(|item| item.summary.clone().unwrap_or_default())
        .collect()
}
