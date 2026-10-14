use super::*;
use futures::task::noop_waker;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::task::Context;

#[derive(Default)]
struct EmptyExtractor {
    visited: Arc<AtomicUsize>,
}

impl Extractor for EmptyExtractor {
    fn article_urls(&self, _doc: &Document, _base_url: Option<&Url>) -> Vec<ArticleUrl> {
        self.visited.fetch_add(1, Ordering::SeqCst);
        Vec::new()
    }
}

fn empty_stream(extractor: EmptyExtractor) -> SourceArticleStream<EmptyExtractor> {
    let visited = extractor.visited.clone();
    let stream = Extrablatt {
        client: Client::new(),
        language: Language::default(),
        main_page: Document::from("<html></html>"),
        main_source: Some("<html></html>".into()),
        main_requested_url: Url::parse("http://fixture.invalid/").unwrap(),
        download_budget: crate::download::DownloadBudget::new(None),
        base_url: Url::parse("http://fixture.invalid/").unwrap(),
        extractor,
        articles: FnvHashMap::default(),
        categories: FnvHashMap::default(),
        config: Config::default(),
    }
    .into_stream_with_source();
    visited.store(0, Ordering::SeqCst);
    stream
}

fn ready_category(stream: &mut SourceArticleStream<EmptyExtractor>, path: &str) {
    let url = Url::parse(&format!("http://fixture.invalid/{path}")).unwrap();
    stream.discover(url.clone(), true);
    stream.pending.clear();
    stream.active.push(Box::pin(async move {
        (
            url.clone(),
            Ok(crate::download::DownloadedHtml {
                url,
                html: "<html></html>".into(),
            }),
        )
    }));
}

#[tokio::test]
async fn empty_downloaded_category_finishes_without_another_wakeup() {
    let mut stream = empty_stream(EmptyExtractor::default());
    ready_category(&mut stream, "empty");
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    assert!(matches!(
        Pin::new(&mut stream).poll_next(&mut context),
        Poll::Ready(None)
    ));
}

#[tokio::test]
async fn empty_category_does_not_leave_later_categories_unpolled() {
    let extractor = EmptyExtractor::default();
    let visited = extractor.visited.clone();
    let mut stream = empty_stream(extractor);
    for path in ["first", "second"] {
        ready_category(&mut stream, path);
    }
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    assert!(matches!(
        Pin::new(&mut stream).poll_next(&mut context),
        Poll::Ready(None)
    ));
    assert_eq!(visited.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn all_cached_categories_are_visited_before_stream_ends() {
    let extractor = EmptyExtractor::default();
    let visited = extractor.visited.clone();
    let mut stream = empty_stream(extractor);
    for path in ["first", "second"] {
        let url = Url::parse(&format!("http://fixture.invalid/{path}")).unwrap();
        stream.cache.insert(
            url.clone(),
            CachedPage {
                url: url.clone(),
                doc: Some(Document::from("<html></html>")),
                raw_html: None,
            },
        );
        stream.discover(url, true);
    }
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    assert!(matches!(
        Pin::new(&mut stream).poll_next(&mut context),
        Poll::Ready(None)
    ));
    assert_eq!(visited.load(Ordering::SeqCst), 2);
}
