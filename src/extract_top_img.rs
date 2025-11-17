use select::document::Document;
use select::predicate::{Attr, Name, Predicate};
use url::Url;
use crate::extract_meta::meta_content;

/// Extract the 'top img' as specified by the website.
pub fn meta_img_url(doc: &Document, base_url: Option<&Url>) -> Option<Url> {
    let options = Url::options().base_url(base_url);
    if let Some(meta) = meta_content(doc, Attr("property", "og:image")) {
        if let Ok(url) = options.parse(&*meta) {
            return Some(url);
        }
    }
    doc.find(
        Name("link").and(
            Attr("rel", "img_src")
                .or(Attr("rel", "image_src"))
                .or(Attr("rel", "icon")),
        ),
    )
    .filter_map(|node| node.attr("href"))
    .filter_map(|href| options.parse(href).ok())
    .next()
}