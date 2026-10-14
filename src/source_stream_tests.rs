use super::*;
use select::predicate::Attr;
use std::{
    borrow::Cow,
    collections::HashMap,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

#[derive(Default)]
struct FixtureExtractor;
impl Extractor for FixtureExtractor {
    fn article_urls(&self, doc: &Document, base: Option<&Url>) -> Vec<ArticleUrl> {
        doc.find(Attr("data-article", "1"))
            .filter_map(|n| base?.join(n.attr("href")?).ok())
            .map(ArticleUrl::new)
            .collect()
    }
    fn categories(&self, doc: &Document, base: &Url) -> Vec<Category> {
        doc.find(Attr("data-category", "1"))
            .filter_map(|n| base.join(n.attr("href")?).ok())
            .map(Category::new)
            .collect()
    }
    fn article_content<'a>(
        &self,
        doc: &'a Document,
        _base: Option<&Url>,
        _language: Option<Language>,
    ) -> ArticleContent<'a> {
        let mut content = ArticleContent::builder();
        if let Some(node) = doc.find(Attr("id", "text")).next() {
            content = content.text(Cow::Owned(node.text()));
        }
        content.build()
    }
}

struct Reply {
    status: u16,
    body: Vec<u8>,
    content_type: &'static str,
    location: Option<&'static str>,
    delay: Duration,
}
impl Reply {
    fn html(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into().into_bytes(),
            content_type: "text/html; charset=utf-8",
            location: None,
            delay: Duration::ZERO,
        }
    }
}
struct Fixture {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    counts: Arc<Mutex<HashMap<String, usize>>>,
    max_active: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    listener: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new(handler: impl Fn(&str) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stopped = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let counts = Arc::new(Mutex::new(HashMap::new()));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let handler = Arc::new(handler);
        let (stop, reqs, count, max) = (
            stopped.clone(),
            requests.clone(),
            counts.clone(),
            max_active.clone(),
        );
        let listener = thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        let (handler, reqs, count, active, max) = (
                            handler.clone(),
                            reqs.clone(),
                            count.clone(),
                            active.clone(),
                            max.clone(),
                        );
                        thread::spawn(move || {
                            socket
                                .set_read_timeout(Some(Duration::from_secs(2)))
                                .unwrap();
                            let mut bytes = vec![0; 8192];
                            let Ok(size) = socket.read(&mut bytes) else {
                                return;
                            };
                            let request = String::from_utf8_lossy(&bytes[..size]).to_string();
                            let target = request.split_whitespace().nth(1).unwrap_or("/");
                            let path = Url::parse(target)
                                .map(|u| u.path().to_string())
                                .unwrap_or_else(|_| target.to_string());
                            reqs.lock().unwrap().push(request);
                            *count.lock().unwrap().entry(path.clone()).or_insert(0) += 1;
                            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                            max.fetch_max(current, Ordering::SeqCst);
                            let reply = handler(&path);
                            thread::sleep(reply.delay);
                            let location = reply
                                .location
                                .map(|value| format!("Location: {value}\r\n"))
                                .unwrap_or_default();
                            let response = format!(
                                "HTTP/1.1 {} OK\r\nContent-Type: {}\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n",
                                reply.status,
                                reply.content_type,
                                reply.body.len(),
                                location
                            );
                            let _ = socket.write_all(response.as_bytes());
                            let _ = socket.write_all(&reply.body);
                            active.fetch_sub(1, Ordering::SeqCst);
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        });
        Self {
            url,
            requests,
            counts,
            max_active,
            stopped,
            listener: Some(listener),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.listener.take().unwrap().join().unwrap();
    }
}

async fn source(fixture: &Fixture, config: Config) -> SourceArticleStream<FixtureExtractor> {
    Extrablatt::builder(format!("{}/", fixture.url))
        .unwrap()
        .no_system_proxy()
        .config(config)
        .build_with_extractor(FixtureExtractor)
        .await
        .unwrap()
        .into_stream_with_source()
}

#[tokio::test]
async fn stream_downloads_overlap_once_and_preserves_raw_source_and_ua() {
    let fixture = Fixture::new(|path| match path {
        "/" => Reply::html(
            "<a data-category='1' data-article='1' href='/shared#first'>shared</a><a data-article='1' href='/shared#second'>same</a>",
        ),
        "/shared" => Reply::html(
            "<div id='text'>shared article text</div><a data-article='1' href='/leaf'>leaf</a>",
        ),
        "/leaf" => Reply::html("<div id='text'>leaf article text</div>"),
        _ => unreachable!(),
    });
    let mut stream = source(
        &fixture,
        Config::builder()
            .concurrency(2)
            .user_agent("fixture-agent")
            .build(),
    )
    .await;
    let mut articles = Vec::new();
    while let Some(item) = stream.next().await {
        articles.push(item.unwrap());
    }
    assert_eq!(articles.len(), 2);
    assert!(
        articles
            .iter()
            .all(|item| item.raw_html.as_ref().unwrap().contains("id='text'"))
    );
    assert_eq!(stream.stats().discovered_articles, 2);
    assert_eq!(fixture.counts.lock().unwrap().values().sum::<usize>(), 3);
    assert_eq!(fixture.counts.lock().unwrap()["/shared"], 1);
    assert!(
        fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.to_lowercase().contains("user-agent: fixture-agent"))
    );
}

#[tokio::test]
async fn stream_limits_actual_concurrent_requests() {
    let fixture = Fixture::new(|path| {
        if path == "/" {
            return Reply::html(
                (0..6)
                    .map(|n| format!("<a data-article='1' href='/{n}'>article</a>"))
                    .collect::<String>(),
            );
        }
        let mut reply = Reply::html("<div id='text'>article text</div>");
        reply.delay = Duration::from_millis(60);
        reply
    });
    let mut stream = source(&fixture, Config::builder().concurrency(2).build()).await;
    let mut count = 0;
    while let Some(item) = stream.next().await {
        item.unwrap();
        count += 1;
    }
    assert_eq!(count, 6);
    assert_eq!(fixture.max_active.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn stream_classifies_failed_categories_articles_and_filtered_text() {
    let fixture = Fixture::new(|path| match path {
        "/" => Reply::html(
            "<a data-category='1' href='/badcategory'>category</a><a data-article='1' href='/bad'>bad</a><a data-article='1' href='/empty'>empty</a><a data-article='1' href='/short'>short</a>",
        ),
        "/badcategory" | "/bad" => {
            let mut reply = Reply::html("error");
            reply.status = 503;
            reply
        }
        "/empty" => Reply::html("<html></html>"),
        "/short" => Reply::html("<div id='text'>one</div>"),
        _ => unreachable!(),
    });
    let mut stream = source(&fixture, Config::builder().min_word_count(5).build()).await;
    let mut errors = 0;
    while let Some(item) = stream.next().await {
        assert!(item.is_err());
        errors += 1;
    }
    assert_eq!(errors, 4);
    assert_eq!(stream.stats().failed_categories, 1);
    assert_eq!(stream.stats().failed_articles, 2);
    assert_eq!(stream.stats().filtered_articles, 1);
}

#[tokio::test]
async fn redirected_category_uses_final_url_for_relative_links() {
    let fixture = Fixture::new(|path| match path {
        "/" => Reply::html("<a data-category='1' href='/redirect'>category</a>"),
        "/redirect" => {
            let mut reply = Reply::html("");
            reply.status = 302;
            reply.location = Some("/section/");
            reply
        }
        "/section/" => Reply::html("<a data-article='1' href='item'>article</a>"),
        "/section/item" => Reply::html("<div id='text'>article text</div>"),
        _ => unreachable!("{path}"),
    });
    let mut stream = source(&fixture, Config::default()).await;
    let article = stream.next().await.unwrap().unwrap();
    assert!(article.article.url.as_str().ends_with("/section/item"));
    assert!(stream.next().await.is_none());
    assert_eq!(fixture.counts.lock().unwrap()["/section/item"], 1);
}

#[tokio::test]
async fn stream_enforces_per_page_total_and_url_budgets() {
    let fixture = Fixture::new(|path| match path {
        "/" => Reply::html("<a data-article='1' href='/large'>article</a>"),
        "/large" => Reply::html(format!("<div id='text'>{}</div>", "text ".repeat(100))),
        _ => unreachable!(),
    });
    let mut stream = source(&fixture, Config::builder().max_response_bytes(100).build()).await;
    assert!(matches!(
        stream.next().await.unwrap(),
        Err(ExtrablattError::LimitExceeded {
            resource: "response_bytes",
            ..
        })
    ));
    assert_eq!(stream.stats().failed_articles, 1);
    let mut stream = source(&fixture, Config::builder().max_total_bytes(100).build()).await;
    assert!(matches!(
        stream.next().await.unwrap(),
        Err(ExtrablattError::LimitExceeded {
            resource: "total_bytes",
            ..
        })
    ));
    assert!(stream.next().await.is_none());
    assert!(stream.stats().downloaded_bytes <= 100);
    let mut stream = source(&fixture, Config::builder().max_article_urls(0).build()).await;
    assert!(matches!(
        stream.next().await.unwrap(),
        Err(ExtrablattError::LimitExceeded {
            resource: "article_urls",
            ..
        })
    ));
}

#[tokio::test]
async fn cached_document_has_no_raw_source_and_is_not_downloaded_again() {
    let fixture = Fixture::new(|_| Reply::html("<a data-article='1' href='/cached'>cached</a>"));
    let mut paper = Extrablatt::builder(format!("{}/", fixture.url))
        .unwrap()
        .no_system_proxy()
        .build_with_extractor(FixtureExtractor)
        .await
        .unwrap();
    let url = Url::parse(&format!("{}/cached", fixture.url)).unwrap();
    paper.articles.insert(
        ArticleUrl::new(url),
        DocumentDownloadState::Success {
            received: Instant::now(),
            doc: Document::from("<div id='text'>cached content</div>"),
        },
    );
    let mut stream = paper.into_stream_with_source();
    let article = stream.next().await.unwrap().unwrap();
    assert!(article.raw_html.is_none());
    assert!(stream.next().await.is_none());
    assert_eq!(fixture.counts.lock().unwrap().values().sum::<usize>(), 1);
}

#[tokio::test]
async fn download_charset_proxy_and_request_timeout_are_applied() {
    let fixture = Fixture::new(|path| {
        let mut reply = Reply::html("");
        reply.body = b"<div id='text'>\xcf\xf0\xe8\xe2\xe5\xf2</div>".to_vec();
        reply.content_type = "text/html; charset=windows-1251";
        if path == "/slow" {
            reply.delay = Duration::from_millis(150);
        }
        reply
    });
    let client = Client::builder().no_proxy().build().unwrap();
    let page = crate::download::fetch_html(&client, Url::parse(&fixture.url).unwrap(), Some(100))
        .await
        .unwrap();
    assert!(page.html.contains("Привет"));
    let timeout_client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(25))
        .build()
        .unwrap();
    assert!(
        matches!(crate::download::fetch_html(&timeout_client, Url::parse(&format!("{}/slow", fixture.url)).unwrap(), None).await, Err(ExtrablattError::HttpRequestFailure { error }) if error.is_timeout())
    );
    let paper = Extrablatt::builder("http://fixture.invalid/")
        .unwrap()
        .proxy(&fixture.url)
        .config(
            Config::builder()
                .request_timeout(Duration::from_secs(2))
                .build(),
        )
        .build_with_extractor(FixtureExtractor)
        .await
        .unwrap();
    assert!(paper.main_source.unwrap().contains("Привет"));
    assert!(
        fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.starts_with("GET http://fixture.invalid/"))
    );
}

#[tokio::test]
async fn invalid_concurrency_fails_before_network_request() {
    let fixture = Fixture::new(|_| panic!("must not make a request"));
    let result = Extrablatt::builder(&fixture.url)
        .unwrap()
        .config(Config::builder().concurrency(0).build())
        .build()
        .await;
    assert!(matches!(result, Err(ExtrablattError::InvalidConfig { .. })));
    assert!(fixture.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn refreshed_homepage_keeps_current_source_without_another_download() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let fixture = Fixture::new(move |_| {
        let number = observed.fetch_add(1, Ordering::SeqCst);
        Reply::html(format!(
            "<a data-article='1' href='/'>main article</a><div id='text'>version {number}</div>"
        ))
    });
    let mut paper = Extrablatt::builder(&fixture.url)
        .unwrap()
        .no_system_proxy()
        .build_with_extractor(FixtureExtractor)
        .await
        .unwrap();
    paper.refresh_homepage().await.unwrap();
    let mut stream = paper.into_stream_with_source();
    let source = stream.next().await.unwrap().unwrap();
    assert!(source.raw_html.unwrap().contains("version 1"));
    assert_eq!(source.article.content.text.as_deref(), Some("version 1"));
    assert!(stream.next().await.is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn category_url_limit_and_legacy_http_failure_classification_are_preserved() {
    let fixture = Fixture::new(|path| {
        if path == "/" {
            return Reply::html("<a data-category='1' href='/broken'>category</a>");
        }
        let mut reply = Reply::html("unavailable");
        reply.status = 503;
        reply
    });
    let mut stream = source(&fixture, Config::builder().max_category_urls(0).build()).await;
    assert!(matches!(
        stream.next().await.unwrap(),
        Err(ExtrablattError::LimitExceeded {
            resource: "category_urls",
            ..
        })
    ));
    let mut paper = Extrablatt::builder(&fixture.url)
        .unwrap()
        .no_system_proxy()
        .build_with_extractor(FixtureExtractor)
        .await
        .unwrap();
    let category = Category::new(Url::parse(&format!("{}/broken", fixture.url)).unwrap());
    assert!(matches!(
        paper.download_category(category.clone()).await,
        Err(ExtrablattError::NoHttpSuccessResponse { .. })
    ));
    assert!(paper.categories()[&category].is_no_http_success_response());
}
