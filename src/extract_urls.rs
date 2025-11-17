use std::borrow::Cow;
use std::collections::HashSet;
use select::document::Document;
use select::predicate::{Name};

/// Extract the `href` attribute for all `<a>` tags of the document.
pub fn all_urls<'a>(doc: &'a Document) -> Vec<Cow<'a, str>> {
    let mut uniques = HashSet::new();
    doc.find(Name("a"))
        .filter_map(|n| n.attr("href").map(str::trim))
        .filter(|href| uniques.insert(*href))
        .map(Cow::Borrowed)
        .collect()
}
