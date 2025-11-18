use select::document::Document;
use select::predicate::{Attr, Name, Predicate};
use url::Url;

pub fn favicon(doc: &Document, base_url: &Url) -> Option<Url> {
    let options = Url::options().base_url(Some(base_url));

    doc.find(Name("link").and(Attr("rel", "icon")))
        .filter_map(|node| node.attr("href"))
        .filter_map(|href| options.parse(href).ok())
        .next()
}
