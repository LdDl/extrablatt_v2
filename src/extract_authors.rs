use std::borrow::Cow;
use std::collections::HashSet;
use regex::Regex;
use select::document::Document;
use lazy_static::lazy_static;

use crate::text::{author_text};

/// Author extraction constants (from newspaper4k)
const AUTHOR_ATTRS: [&str; 6] = ["name", "rel", "itemprop", "class", "id", "property"];
const AUTHOR_VALS: [&str; 12] = [
    "author", "byline", "dc.creator", "byl", "article:author", "article:author_name",
    "story-byline", "article-author", "parsely-author", "sailthru.author", "citation_author",
    "article-author"
];
const AUTHOR_STOP_WORDS: [&str; 13] = [
    "By", "Reuters", "IANS", "AP", "AFP", "PTI", "ANI", "DPA", "Senior Reporter",
    "Reporter", "Writer", "Opinion Writer", "Opinion Writer"
];

lazy_static! {
    /// Regex for cleaning author names.
    ///
    /// This strips leading "By " and also potential profile links.
    static ref RE_AUTHOR_NAME: Regex =
        Regex::new(r"(?mi)(By)?\s*((<|(&lt;))a([^>]*)(>|(&gt;)))?(?P<name>[a-z ,.'-]+)((<|(&lt;))\\/a(>|(&gt;)))?").unwrap();
}

/// Extract all the listed authors for the article.
pub fn authors<'a>(doc: &'a Document) -> Vec<Cow<'a, str>> {
    let mut authors = Vec::new();

    for node in doc.nodes.iter() {
        if let select::node::Data::Element(tag, attrs) = &node.data {
            let tag_name = tag.local.as_ref().to_lowercase();
            if tag_name == "script" || tag_name == "style" || tag_name == "time" {
                continue;
            }
            for (attr_name, attr_value) in attrs.iter() {
                let attr_name_str = attr_name.local.as_ref().to_lowercase();
                let attr_value_str = attr_value.as_ref().to_lowercase();
                for &author_attr in &AUTHOR_ATTRS {
                    if attr_name_str == author_attr {
                        for &author_val in &AUTHOR_VALS {
                            if attr_value_str == author_val || attr_value_str.contains(author_val) {
                                let mut content = String::new();
                                if tag_name == "meta" {
                                    if let Some(c) = attrs.iter().find(|(a, _)| a.local.as_ref().eq_ignore_ascii_case("content")) {
                                        content = c.1.as_ref().to_string();
                                    }
                                }
                                if content.is_empty() {
                                    if let Some(node_ref) = doc.nth(node.index) {
                                        content = author_text(node_ref);
                                    }
                                }
                                for name in parse_byline(&content) {
                                    if !name.is_empty() {
                                        authors.push(name);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Deduplicate and filter (case-insensitive, trimmed)
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for author in authors {
        let a = clean_author(&author);
        let key = a.to_lowercase();
        if is_valid_name(&a) && !a.is_empty() && !seen.contains(&key) {
            seen.insert(key);
            result.push(a);
        }
    }
    // Sort authors alphabetically for deterministic output
    result.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    result.into_iter().map(Cow::Owned).collect()
}

// Helper functions for author extraction
fn clean_author(s: &str) -> String {
    let mut out = s.trim().to_string();
    for stop in AUTHOR_STOP_WORDS.iter() {
        out = out.replace(stop, "");
    }
    // Remove HTML tags
    out = Regex::new(r"<[^>]+>").unwrap().replace_all(&out, "").to_string();
    out.trim_matches(|c: char| c == '.' || c == ',' || c == '-' || c == '/' || c.is_whitespace()).to_string()
}

fn contains_digits(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_digit())
}

fn is_valid_name(s: &str) -> bool {
    let word_count = s.split_whitespace().count();
    word_count > 1 && word_count < 5 && !contains_digits(s) && !s.contains('<') && !s.contains('>')
}

fn parse_byline(s: &str) -> Vec<String> {
    let s = s.replace(['\n', '\t', '\r', '\u{a0}'], " ");
    let mut out = Vec::new();
    for token in s.split(|c| c == '·' || c == ',' || c == '|' || c == '/' || c == '\u{a0}') {
        let t = token.trim();
        if is_valid_name(t) {
            // If more than 2 words, keep only first two (to avoid picking up extra words after name)
            let words: Vec<&str> = t.split_whitespace().collect();
            let name = if words.len() > 2 {
                words[..2].join(" ")
            } else {
                t.to_string()
            };
            out.push(clean_author(&name));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_regex() {
        let m = RE_AUTHOR_NAME
            .captures("By &lt;a href=&quot;/profiles/meg-wagner&quot;&gt;Joseph Kelley&lt;/a&gt;")
            .unwrap()
            .name("name")
            .unwrap();
        assert_eq!(m.as_str(), "Joseph Kelley");

        let m = RE_AUTHOR_NAME
            .captures("By <a href=&quot;/profiles/meg-wagner&quot;>Joseph Kelley</a>")
            .unwrap()
            .name("name")
            .unwrap();
        assert_eq!(m.as_str(), "Joseph Kelley");

        let m = RE_AUTHOR_NAME
            .captures("Joseph Kelley")
            .unwrap()
            .name("name")
            .unwrap();
        assert_eq!(m.as_str(), "Joseph Kelley");

        let m = RE_AUTHOR_NAME
            .captures("By Joseph Kelley")
            .unwrap()
            .name("name")
            .unwrap();
        assert_eq!(m.as_str(), "Joseph Kelley");

        let m = RE_AUTHOR_NAME
            .captures("J\'oseph-Kelley")
            .unwrap()
            .name("name")
            .unwrap();
        assert_eq!(m.as_str(), "J\'oseph-Kelley");
    }
}