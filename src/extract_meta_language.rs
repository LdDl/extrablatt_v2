use std::str::FromStr;
use select::document::Document;
use select::predicate::{Attr, Name};
use crate::Language;
use crate::extract_meta::meta_content;

/// Extract content language from meta tag or html lang attribute.
pub fn meta_language(doc: &Document) -> Option<Language> {
    let mut unknown_lang: Option<Language> = None;

    // First, check the standard html lang attribute
    if let Some(html_tag) = doc.find(Name("html")).next() {
        if let Some(lang_attr) = html_tag.attr("lang") {
            match Language::from_str(lang_attr) {
                Ok(lang) => return Some(lang),
                Err(lang) => {
                    unknown_lang = Some(lang);
                }
            }
        }
    }

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