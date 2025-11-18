use std::borrow::Cow;
use std::collections::HashSet;
use url::Url;
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

/// Extract all of the images of the document.
pub fn image_urls(doc: &Document, base_url: Option<&Url>) -> Vec<Url> {
    let options = Url::options().base_url(base_url);
    // TODO extract `picture` and source media
    doc.find(Name("img"))
        .filter_map(|n| n.attr("href").map(str::trim))
        .filter_map(|url| options.parse(url).ok())
        .collect()
}