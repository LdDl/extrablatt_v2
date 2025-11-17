use std::borrow::Cow;
use select::document::Document;
use select::predicate::{Attr, Name};
use crate::extract_meta::meta_content;

const MOTLEY_REPLACEMENT: (&str, &str) = ("&#65533;", "");
const TITLE_REPLACEMENTS: (&str, &str) = ("&raquo;", "»");
const TITLE_META_INFO: [&str; 8] = [
    "dc.title",
    "og:title",
    "headline",
    "articletitle",
    "article-title",
    "parsely-title",
    "title",
    "twitter:title",
];

pub fn title<'a>(doc: &'a Document) -> Option<Cow<'a, str>> {
    // 1. Try og:title/twitter:title first
    for meta_name in &TITLE_META_INFO {
        if let Some(meta) = meta_content(doc, Attr("property", meta_name)) {
            let t = meta.trim();
            if !t.is_empty() {
                return Some(Cow::Owned(postprocess_title(t)));
            }
        }
        if let Some(meta) = meta_content(doc, Attr("name", meta_name)) {
            let t = meta.trim();
            if !t.is_empty() {
                return Some(Cow::Owned(postprocess_title(t)));
            }
        }
    }

    // 2. Try <h1> (longest, >2 words)
    let h1_list: Vec<String> = doc.find(Name("h1")).filter_map(|n| n.as_text().map(|s| s.trim().to_string())).collect();
    if !h1_list.is_empty() {
        let mut sorted = h1_list.clone();
        sorted.sort_by_key(|s| s.len());
        let longest = sorted.last().unwrap();
        if longest.split_whitespace().count() > 2 {
            let processed = postprocess_title(&longest.split_whitespace().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" "));
            return Some(Cow::Owned(processed));
        }
    }

    // 3. Try <title>
    if let Some(title_tag) = doc.find(Name("title")).next().and_then(|n| n.as_text().map(str::trim)) {
        let t = title_tag.trim();
        if !t.is_empty() {
            return Some(Cow::Owned(postprocess_title(t)));
        }
    }

    // 4. Advanced heuristics fallback
    // Re-extract <h1> and <title> for heuristics
    let h1_list: Vec<String> = doc.find(Name("h1")).filter_map(|n| n.as_text().map(|s| s.trim().to_string())).collect();
    let mut title_text_h1 = String::new();
    if !h1_list.is_empty() {
        let mut sorted = h1_list.clone();
        sorted.sort_by_key(|s| s.len());
        let longest = sorted.last().unwrap();
        title_text_h1 = longest.clone();
    }

    let title_tag = doc.find(Name("title")).next().and_then(|n| n.as_text().map(str::trim));
    let mut title_text = title_tag.unwrap_or("").to_string();

    // Get og:title/twitter:title for fallback
    let mut title_text_fb = String::new();
    for meta_name in &TITLE_META_INFO {
        if let Some(meta) = meta_content(doc, Attr("property", meta_name)) {
            title_text_fb = meta.trim().to_string();
            break;
        }
        if let Some(meta) = meta_content(doc, Attr("name", meta_name)) {
            title_text_fb = meta.trim().to_string();
            break;
        }
    }

    // Filtering for comparison (alphanumeric only, lowercased)
    let filter = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let filter_title_text = filter(&title_text);
    let filter_title_text_h1 = filter(&title_text_h1);
    let filter_title_text_fb = filter(&title_text_fb);

    // Heuristics
    let mut candidate = String::new();
    if !title_text_h1.is_empty() && title_text_h1 == title_text {
        candidate = title_text_h1.clone();
    } else if !filter_title_text_h1.is_empty() && filter_title_text_h1 == filter_title_text_fb {
        candidate = title_text_h1.clone();
    } else if !filter_title_text_h1.is_empty()
        && filter_title_text.contains(&filter_title_text_h1)
        && !filter_title_text_fb.is_empty()
        && filter_title_text.contains(&filter_title_text_fb)
        && title_text_h1.len() > title_text_fb.len()
    {
        candidate = title_text_h1.clone();
    } else if !filter_title_text_fb.is_empty()
        && filter_title_text_fb != filter_title_text
        && filter_title_text.starts_with(&filter_title_text_fb)
    {
        candidate = title_text_fb.clone();
    }

    // Delimiter splitting
    if candidate.is_empty() && !title_text.is_empty() {
        for delimiter in ["|", "-", "_", "/", " » "] {
            if title_text.contains(delimiter) {
                let pieces: Vec<&str> = title_text.split(delimiter).collect();
                let mut large_text_length = 0;
                let mut large_text_index = 0;
                let hint = filter(&title_text_h1);
                for (i, piece) in pieces.iter().enumerate() {
                    let current = piece.trim();
                    if !hint.is_empty() && filter(current).contains(&hint) {
                        large_text_index = i;
                        break;
                    }
                    if current.len() > large_text_length {
                        large_text_length = current.len();
                        large_text_index = i;
                    }
                }
                candidate = pieces[large_text_index].trim().to_string();
                break;
            }
        }
    }

    // Final filter: prefer h1 if similar
    let filter_candidate = filter(&candidate);
    if !title_text_h1.is_empty() && filter_title_text_h1 == filter_candidate {
        candidate = title_text_h1.clone();
    }

    if !candidate.is_empty() {
        return Some(Cow::Owned(postprocess_title(&candidate)));
    }
    None
}

fn postprocess_title(title: &str) -> String {
    let mut t = title.replace(MOTLEY_REPLACEMENT.0, MOTLEY_REPLACEMENT.1);
    t = t.replace(TITLE_REPLACEMENTS.0, TITLE_REPLACEMENTS.1);
    t.trim().to_string()
}