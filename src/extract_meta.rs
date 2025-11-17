use std::borrow::Cow;
use select::document::Document;
use select::predicate::{Attr, Name, Predicate};

/// Extract a given meta content form document.
pub fn meta_content<'a, 'b>(
    doc: &'a Document,
    attr: Attr<&'b str, &'b str>,
) -> Option<Cow<'a, str>> {
    doc.find(Name("head").descendant(Name("meta").and(attr)))
        .filter_map(|node| {
            node.attr("content")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(Cow::Borrowed)
        })
        .next()
}