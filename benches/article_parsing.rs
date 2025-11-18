use criterion::{black_box, criterion_group, criterion_main, Criterion};
use extrablatt_v2::{Article, DefaultExtractor, Extractor};
use select::document::Document;
use url::Url;

static HTML: &str = include_str!("../scripts/file.html");
const URL: &str = "https://ya62.ru/text/entertainment/2025/11/15/76123751/";

fn bench_full_article_parsing(c: &mut Criterion) {
    c.bench_function("full_article_parse", |b| {
        b.iter(|| {
            Article::new(black_box(URL), black_box(HTML)).unwrap()
        });
    });
}

fn bench_individual_extractors(c: &mut Criterion) {
    let doc = Document::from(HTML);
    let extractor = DefaultExtractor::default();
    let base_url = Url::parse(URL).ok();

    c.bench_function("extract_title", |b| {
        b.iter(|| {
            extractor.title(black_box(&doc))
        });
    });

    c.bench_function("extract_authors", |b| {
        b.iter(|| {
            extractor.authors(black_box(&doc))
        });
    });

    c.bench_function("extract_text", |b| {
        b.iter(|| {
            extractor.text(black_box(&doc), black_box(extrablatt_v2::Language::Russian))
        });
    });

    c.bench_function("extract_publishing_date", |b| {
        b.iter(|| {
            extractor.publishing_date(black_box(&doc), base_url.as_ref())
        });
    });

    c.bench_function("extract_meta_language", |b| {
        b.iter(|| {
            extractor.meta_language(black_box(&doc))
        });
    });

    // Missing extractors that weren't benchmarked before
    c.bench_function("extract_meta_keywords", |b| {
        b.iter(|| {
            extractor.meta_keywords(black_box(&doc))
        });
    });

    c.bench_function("extract_meta_description", |b| {
        b.iter(|| {
            extractor.meta_description(black_box(&doc))
        });
    });

    c.bench_function("extract_meta_thumbnail", |b| {
        b.iter(|| {
            extractor.meta_thumbnail_url(black_box(&doc), base_url.as_ref())
        });
    });

    c.bench_function("extract_meta_img", |b| {
        b.iter(|| {
            extractor.meta_img_url(black_box(&doc), base_url.as_ref())
        });
    });

    c.bench_function("extract_videos", |b| {
        b.iter(|| {
            extractor.videos(black_box(&doc), Some(extrablatt_v2::Language::Russian))
        });
    });
}

criterion_group!(benches, bench_full_article_parsing, bench_individual_extractors);
criterion_main!(benches);
