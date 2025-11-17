use select::document::Document;
use url::Url;
use crate::date::{ArticleDate, DateExtractor};

/// Extract a publishing date from the document or URL path.
pub fn publishing_date(doc: &Document, base_url: Option<&Url>) -> Option<ArticleDate> {
    if let Some(date) = DateExtractor::extract_from_doc(doc) {
        return Some(date);
    }
    if let Some(url) = base_url {
        return DateExtractor::extract_from_str(url.path());
    }
    None
}