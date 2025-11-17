use std::borrow::Cow;
use select::document::Document;
use select::predicate::{Attr, Name, Predicate};
use crate::extract_meta::meta_content;

/// Extract the article title.
///
/// Assumptions:
///    - `og:title` usually contains the plain title, but shortened compared
///      to `<h1>`
///    - `<title>` tag is the most reliable, but often contains also the
///      newspaper name like: "Some title - The New York Times"
///    - `<h1>`, if properly detected, is the best since this is also
///      displayed to users)
///
///    Matching strategy:
///    1.  `<h1>` takes precedent over `og:title`
///    2. `og:title` takes precedent over `<title>`
pub fn title<'a>(doc: &'a Document) -> Option<Cow<'a, str>> {
    if let Some(title) = doc
        .find(Name("h1"))
        .filter_map(|node| node.as_text().map(str::trim))
        .next()
    {
        return Some(Cow::Borrowed(title));
    }

    if let Some(title) = meta_content(doc, Attr("property", "og:title")) {
        return Some(title);
    }

    if let Some(title) = meta_content(doc, Attr("name", "og:title")) {
        return Some(title);
    }

    if let Some(title) = doc.find(Name("title")).next() {
        return title.as_text().map(str::trim).map(Cow::Borrowed);
    }
    None
}