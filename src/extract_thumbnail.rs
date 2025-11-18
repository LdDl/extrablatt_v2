use select::document::Document;
use select::predicate::{Attr};
use url::Url;
use crate::extract_meta::meta_content;

/// Extract the thumbnail for the article.
pub fn meta_thumbnail_url(doc: &Document, base_url: Option<&Url>) -> Option<Url> {
    let options = Url::options().base_url(base_url);
    [("name", "thumbnail"), ("name", "thumbnailUrl")]
        .iter()
        .filter_map(|(k, v)| meta_content(doc, Attr(k, v)))
        .filter_map(|url| options.parse(&*url).ok())
        .next()
}