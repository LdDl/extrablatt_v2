use reqwest::Url;
use select::document::Document;
use select::predicate::{Attr, Name, Predicate};
use crate::extract_meta::meta_content;

/// Return the article's canonical URL
///
/// Gets the first available value of:
///   1. The rel=canonical tag
///   2. The og:url tag
pub fn canonical_link(doc: &Document) -> Option<Url> {
    if let Some(link) = doc
        .find(Name("link").and(Attr("rel", "canonical")))
        .filter_map(|node| node.attr("href"))
        .next()
    {
        return Url::parse(link).ok();
    }

    if let Some(meta) = meta_content(doc, Attr("property", "og:url")) {
        return Url::parse(&*meta).ok();
    }

    None
}
