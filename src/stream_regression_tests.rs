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

fn empty_stream(extractor: EmptyExtractor) -> ArticleStream<EmptyExtractor> {
    ArticleStream {
        paper: Extrablatt {
            client: Client::new(),
            language: Language::default(),
            main_page: Document::from("<html></html>"),
            base_url: Url::parse("http://fixture.invalid/").unwrap(),
            extractor,
            articles: FnvHashMap::default(),
            categories: FnvHashMap::default(),
            config: Config::default(),
        },
        article_responses: Vec::new(),
        articles: Vec::new(),
        categories: Vec::new(),
        category_responses: Vec::new(),
    }
}

#[tokio::test]
async fn empty_downloaded_category_finishes_without_another_wakeup() {
    let mut stream = empty_stream(EmptyExtractor::default());
    stream.category_responses.push(Box::pin(async {
        Ok((
            Url::parse("http://fixture.invalid/empty").unwrap(),
            Bytes::from_static(b"<html></html>"),
        ))
    }));
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
        stream.category_responses.push(Box::pin(async move {
            Ok((
                Url::parse(&format!("http://fixture.invalid/{path}")).unwrap(),
                Bytes::from_static(b"<html></html>"),
            ))
        }));
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
        stream.categories.push((
            Category::new(Url::parse(&format!("http://fixture.invalid/{path}")).unwrap()),
            Document::from("<html></html>"),
        ));
    }
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    assert!(matches!(
        Pin::new(&mut stream).poll_next(&mut context),
        Poll::Ready(None)
    ));
    assert_eq!(visited.load(Ordering::SeqCst), 2);
}
