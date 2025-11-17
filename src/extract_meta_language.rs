use std::str::FromStr;
use select::document::Document;
use select::predicate::{Attr};
use crate::Language;
use crate::extract_meta::meta_content;

/// Extract content language from meta tag.
pub fn meta_language(doc: &Document) -> Option<Language> {
    let mut unknown_lang: Option<Language> = None;
    if let Some(meta) = meta_content(doc, Attr("http-equiv", "Content-Language")) {
        match Language::from_str(&*meta) {
            Ok(lang) => return Some(lang),
            Err(lang) => {
                unknown_lang = Some(lang);
            }
        }
    }
    if let Some(meta) = meta_content(doc, Attr("name", "lang")) {
        match Language::from_str(&*meta) {
            Ok(lang) => return Some(lang),
            Err(lang) => {
                unknown_lang = Some(lang);
            }
        }
    }
    unknown_lang
}