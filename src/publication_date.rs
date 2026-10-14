use chrono::{DateTime, NaiveDate, NaiveDateTime};
use select::predicate::Name;

use crate::article::Article;
use crate::date::Date;

fn iso_date(value: &str) -> Option<String> {
    let value = value.trim();
    if let Ok(date) = DateTime::parse_from_rfc3339(value) {
        return Some(date.to_rfc3339());
    }
    if let Ok(date) = DateTime::parse_from_rfc2822(value) {
        return Some(date.to_rfc3339());
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f%z", "%Y-%m-%d %H:%M:%S%.f%:z"] {
        if let Ok(date) = DateTime::parse_from_str(value, format) {
            return Some(date.to_rfc3339());
        }
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(date) = NaiveDateTime::parse_from_str(value, format) {
            return Some(date.format("%Y-%m-%dT%H:%M:%S%.f").to_string());
        }
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .map(|date| date.format("%Y-%m-%d").to_string())
}

fn json_publication_date(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(object) => object
            .get("datePublished")
            .and_then(serde_json::Value::as_str)
            .and_then(iso_date)
            .or_else(|| {
                object
                    .get("dateCreated")
                    .and_then(serde_json::Value::as_str)
                    .and_then(iso_date)
            })
            .or_else(|| object.get("@graph").and_then(json_publication_date)),
        serde_json::Value::Array(items) => items.iter().find_map(json_publication_date),
        _ => None,
    }
}

pub(crate) fn publication_date_iso8601(article: &Article) -> Option<String> {
    // Source timestamps retain offsets that the legacy naive date type cannot store.
    const DATE_NAMES: &[&str] = &[
        "article:published_time",
        "rnews:datepublished",
        "article:published",
        "og:published_time",
        "datepublished",
        "originalpublicationdate",
        "article_date_original",
        "publication_date",
        "sailthru.date",
        "publishdate",
        "pubdate",
        "publish_date",
        "datecreated",
    ];
    for date_name in DATE_NAMES {
        for node in article.doc.find(Name("meta")) {
            let is_publication = ["property", "name", "itemprop"]
                .iter()
                .filter_map(|attribute| node.attr(attribute))
                .any(|name| name.eq_ignore_ascii_case(date_name));
            if is_publication {
                if let Some(date) = node
                    .attr("content")
                    .and_then(iso_date)
                    .or_else(|| node.attr("datetime").and_then(iso_date))
                {
                    return Some(date);
                }
            }
        }
    }
    for node in article.doc.find(Name("script")) {
        if node
            .attr("type")
            .is_some_and(|kind| kind.eq_ignore_ascii_case("application/ld+json"))
        {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&node.text()) {
                if let Some(date) = json_publication_date(&value) {
                    return Some(date);
                }
            }
        }
    }
    for node in article.doc.find(Name("time")) {
        if node
            .attr("itemprop")
            .is_some_and(|value| value.eq_ignore_ascii_case("dateModified"))
        {
            continue;
        }
        if let Some(date) = node.attr("datetime").and_then(iso_date) {
            return Some(date);
        }
    }
    article
        .content
        .publishing_date
        .as_ref()
        .map(|date| match &date.published {
            Date::DateTime(value) => value.format("%Y-%m-%dT%H:%M:%S").to_string(),
            Date::Date(value) => value.format("%Y-%m-%d").to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn article(html: &str) -> Article {
        Article::new("https://example.com/news/story", html).unwrap()
    }

    #[test]
    fn metadata_preserves_explicit_offset() {
        let value =
            article("<meta property='article:published_time' content='2026-10-14T15:30:00+03:00'>");
        assert_eq!(
            value.publication_date_iso8601().as_deref(),
            Some("2026-10-14T15:30:00+03:00")
        );
    }

    #[test]
    fn json_ld_graph_preserves_negative_offset() {
        let value = article(
            r#"<script type="application/ld+json">{"@graph":[{"datePublished":"2026-10-14T08:30:00-04:00"}]}</script>"#,
        );
        assert_eq!(
            value.publication_date_iso8601().as_deref(),
            Some("2026-10-14T08:30:00-04:00")
        );
    }

    #[test]
    fn naive_dates_do_not_invent_timezone() {
        let value =
            article("<meta property='article:published_time' content='2026-10-14T15:30:00'>");
        assert_eq!(
            value.publication_date_iso8601().as_deref(),
            Some("2026-10-14T15:30:00")
        );
    }

    #[test]
    fn date_only_stays_date_only() {
        let value = article("<meta property='article:published_time' content='2026-10-14'>");
        assert_eq!(
            value.publication_date_iso8601().as_deref(),
            Some("2026-10-14")
        );
    }

    #[test]
    fn time_tag_preserves_offset_and_skips_modification_date() {
        let value = article(
            "<time itemprop='dateModified' datetime='2026-10-15T12:00:00+03:00'></time><time itemprop='datePublished' datetime='2026-10-14T12:00:00+03:00'></time>",
        );
        assert_eq!(
            value.publication_date_iso8601().as_deref(),
            Some("2026-10-14T12:00:00+03:00")
        );
    }

    #[test]
    fn missing_dates_stay_missing() {
        assert_eq!(
            article("<html><body>No date</body></html>").publication_date_iso8601(),
            None
        );
    }

    #[test]
    fn legacy_date_is_used_if_source_has_no_date() {
        let mut value = article("<html></html>");
        value.content.publishing_date = Some(crate::date::ArticleDate {
            published: Date::Date(NaiveDate::from_ymd_opt(2026, 10, 14).unwrap()),
            last_updated: None,
        });
        assert_eq!(
            value.publication_date_iso8601().as_deref(),
            Some("2026-10-14")
        );
    }
    #[test]
    fn publication_date_takes_precedence_over_creation_date() {
        let value = article(
            "<meta name='dateCreated' content='2026-10-13T15:30:00+03:00'><meta property='article:published_time' content='2026-10-14T15:30:00+03:00'>",
        );
        assert_eq!(
            value.publication_date_iso8601().as_deref(),
            Some("2026-10-14T15:30:00+03:00")
        );
    }
}
