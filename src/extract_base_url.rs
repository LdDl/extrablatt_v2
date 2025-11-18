use reqwest::Url;
use select::document::Document;
use select::predicate::Name;

/// Finds the href in the `<base>` tag.
pub fn base_url(doc: &Document) -> Option<Url> {
    doc.find(Name("base"))
        .filter_map(|n| n.attr("href"))
        .filter_map(|href| Url::parse(href).ok())
        .next()
}
