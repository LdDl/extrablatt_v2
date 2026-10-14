use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::time::Duration;

use fnv::FnvHashMap;
use futures::stream::{self, Stream};
use futures::task::Poll;
use futures::{Future, StreamExt};
use reqwest::Response;
use reqwest::header::HeaderMap;
#[cfg(not(target_arch = "wasm32"))]
use reqwest::header::USER_AGENT;
use reqwest::{Client, IntoUrl, Url};
use select::document::Document;
use wasm_timer::Instant;

use crate::error::ExtrablattError;

use crate::Category;
use crate::article::{Article, ArticleContent, ArticleUrl, PureArticle};
use crate::extract::{DefaultExtractor, Extractor};
use crate::language::Language;
use crate::text::ArticleTextNodeExtractor;

/// Caches article downloads
#[derive(Debug)]
pub struct Extrablatt<TExtractor: Extractor = DefaultExtractor> {
    /// The [`reqwest::Client`] that drives requests.
    client: Client,
    /// The expected language of this newspaper.
    language: Language,
    /// The parsed main page.
    main_page: Document,
    main_source: Option<String>,
    main_requested_url: Url,
    download_budget: crate::download::DownloadBudget,
    /// Url of the main page.
    base_url: Url,
    /// The [`crate::Extractor`] used for content retrieval.
    ///
    /// Default is [`crate::DefaultExtractor`].
    extractor: TExtractor,
    /// Cache for retrieved articles.
    articles: FnvHashMap<ArticleUrl, DocumentDownloadState>,
    /// All known categories for this newspaper.
    categories: FnvHashMap<Category, DocumentDownloadState>,
    /// Configuration for article extraction.
    config: Config,
}

impl Extrablatt<DefaultExtractor> {
    /// Convenience method for creating a new [`ExtrablattBuilder`].
    ///
    /// Same as calling [`ExtrablattBuilder::new`].
    #[inline]
    pub fn builder<T: IntoUrl>(url: T) -> Result<ExtrablattBuilder, ExtrablattError> {
        ExtrablattBuilder::new(url)
    }
}

impl<TExtractor: Extractor> Extrablatt<TExtractor> {
    #[inline]
    pub fn language(&self) -> &Language {
        &self.language
    }

    /// The used to store and detect valid articles.
    #[inline]
    pub fn config(&self) -> &Config {
        &self.config
    }

    #[inline]
    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }

    /// The extractor used to retrieve content for an article.
    #[inline]
    pub fn extractor(&self) -> &TExtractor {
        &self.extractor
    }

    /// All available categories.
    #[inline]
    pub fn categories(&self) -> &FnvHashMap<Category, DocumentDownloadState> {
        &self.categories
    }

    /// All available articles.
    #[inline]
    pub fn articles(&self) -> &FnvHashMap<ArticleUrl, DocumentDownloadState> {
        &self.articles
    }

    /// Clear all cached articles and categories.
    #[inline]
    pub fn clear(&mut self) {
        self.articles.clear();
        self.categories.clear()
    }

    /// Insert all categories extracted from the main page.
    fn insert_new_categories(&mut self) {
        for category in self.extractor.categories(&self.main_page, &self.base_url) {
            self.categories
                .entry(category)
                .or_insert(DocumentDownloadState::NotRequested);
        }
    }

    /// Download and store all outstanding articles and returns an iterator over
    /// their results.
    ///
    /// # Example
    ///
    /// Loop over all downloaded articles.
    ///
    /// ```no_run
    /// # use extrablatt_v2::Extrablatt;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let mut site = Extrablatt::builder("https://cnn.com/")?.build().await?;
    ///     site.download_all_remaining_categories().await;
    ///     for(url, content) in site.download_articles().await.successes() {
    ///         // ...
    ///     }
    /// #   Ok(())
    /// # }
    /// ```
    pub async fn download_articles(&mut self) -> ArticleDownloadIter<'_, TExtractor> {
        let this = &*self;
        let results = stream::iter(
            self.articles
                .iter()
                .filter_map(|(article, state)| {
                    if state.is_not_requested() {
                        Some(article.url.clone())
                    } else {
                        None
                    }
                })
                .map(|url| async move {
                    let result = this.get_document(url.clone()).await;
                    (url, result)
                }),
        )
        .buffer_unordered(self.config.concurrency)
        .collect::<Vec<_>>()
        .await;

        for (url, doc) in results {
            let state = match doc {
                Ok((doc, received)) => DocumentDownloadState::Success { received, doc },
                Err((state, err)) => {
                    if !self.config.http_success_only {
                        if let Ok((doc, received)) =
                            DocumentDownloadState::advance_non_http_success(err).await
                        {
                            DocumentDownloadState::Success { doc, received }
                        } else {
                            state
                        }
                    } else {
                        state
                    }
                }
            };

            *self.articles.get_mut(&url).unwrap() = state;
        }

        ArticleDownloadIter {
            inner: self.articles.iter(),
            extractor: &self.extractor,
            language: self.language.clone(),
            base_url: &self.base_url,
        }
    }

    /// Iterator over all known articles.
    pub fn iter_articles(&self) -> ArticleDownloadIter<'_, TExtractor> {
        ArticleDownloadIter {
            inner: self.articles.iter(),
            extractor: &self.extractor,
            language: self.language.clone(),
            base_url: &self.base_url,
        }
    }

    /// For each successfully downloaded category document, insert their article
    /// urls as unrequested.
    fn insert_article_urls(&mut self, doc: &Document) {
        for url in self.extractor.article_urls(doc, Some(&self.base_url)) {
            self.articles
                .entry(url)
                .or_insert(DocumentDownloadState::NotRequested);
        }
    }

    /// Add a category to the pool and downloads it's content.
    ///
    /// If the category is already available it's content is only requested if
    /// it hasn't been requested or a previous request failed.
    pub async fn download_category(
        &mut self,
        category: Category,
    ) -> Result<&Document, ExtrablattError> {
        if self
            .categories
            .get(&category)
            .and_then(|s| s.success_document())
            .is_some()
        {
            return Ok(self.categories[&category].success_document().unwrap());
        }

        let result = match self.get_document(category.url.clone()).await {
            Ok((doc, received)) => {
                self.insert_article_urls(&doc);
                Ok((doc, received))
            }
            Err((state, err)) => {
                if !self.config.http_success_only {
                    match DocumentDownloadState::advance_non_http_success(err).await {
                        Ok((doc, received)) => {
                            self.insert_article_urls(&doc);
                            Ok((doc, received))
                        }
                        Err(err) => Err((state, err)),
                    }
                } else {
                    Err((state, err))
                }
            }
        };
        match result {
            Ok((doc, received)) => {
                self.categories.remove(&category);
                Ok(self
                    .categories
                    .entry(category)
                    .or_insert(DocumentDownloadState::Success { doc, received })
                    .success_document()
                    .unwrap())
            }
            Err((state, err)) => {
                self.categories.insert(category, state);
                Err(err)
            }
        }
    }

    /// Download all categories and extract the article urls
    async fn download_categories(
        &mut self,
        items: Vec<Category>,
    ) -> Vec<Result<Category, (Category, ExtrablattError)>> {
        let this = &*self;
        let requests = stream::iter(items.into_iter().map(|cat| async move {
            let result = this.get_document(cat.url.clone()).await;
            (cat, result)
        }))
        .buffer_unordered(self.config.concurrency)
        .collect::<Vec<_>>()
        .await;
        let mut results = Vec::with_capacity(requests.len());

        for (cat, res) in requests {
            let res = match res {
                Ok((doc, received)) => {
                    self.insert_article_urls(&doc);
                    *self.categories.get_mut(&cat).unwrap() =
                        DocumentDownloadState::Success { doc, received };
                    Ok(cat)
                }
                Err((state, err)) => {
                    if !self.config.http_success_only {
                        match DocumentDownloadState::advance_non_http_success(err).await {
                            Ok((doc, received)) => {
                                *self.categories.get_mut(&cat).unwrap() =
                                    DocumentDownloadState::Success { doc, received };
                                Ok(cat)
                            }
                            Err(err) => {
                                *self.categories.get_mut(&cat).unwrap() = state;
                                Err((cat, err))
                            }
                        }
                    } else {
                        *self.categories.get_mut(&cat).unwrap() = state;
                        Err((cat, err))
                    }
                }
            };
            results.push(res);
        }
        results
    }

    /// Retry downloading all missing category documents, including failed
    /// previous attempts.
    pub async fn retry_download_categories(
        &mut self,
    ) -> Vec<Result<Category, (Category, ExtrablattError)>> {
        let items: Vec<_> = self
            .categories
            .iter()
            .filter_map(|(cat, state)| {
                if !state.is_success() {
                    Some(cat.clone())
                } else {
                    None
                }
            })
            .collect();
        self.download_categories(items).await
    }

    /// Download and store all categories and their identified articles that
    /// haven't been requested yet.
    pub async fn download_all_remaining_categories(
        &mut self,
    ) -> Vec<Result<Category, (Category, ExtrablattError)>> {
        let items: Vec<_> = self
            .categories
            .iter()
            .filter_map(|(cat, state)| {
                if state.is_not_requested() {
                    Some(cat.clone())
                } else {
                    None
                }
            })
            .collect();
        self.download_categories(items).await
    }

    /// Refresh the main page, insert new categories and return the old
    /// document.
    pub async fn refresh_homepage(&mut self) -> Result<Document, ExtrablattError> {
        let downloaded = crate::download::fetch_html_with_budget(
            &self.client,
            self.base_url.clone(),
            self.config.max_response_bytes,
            &self.download_budget,
            self.config.http_success_only,
        )
        .await?;
        self.main_requested_url = self.base_url.clone();
        self.base_url = normalize_url(downloaded.url);
        let old = std::mem::replace(
            &mut self.main_page,
            Document::from(downloaded.html.as_str()),
        );
        self.main_source = Some(downloaded.html);
        self.insert_new_categories();
        Ok(old)
    }

    /// Execute a GET request and return the response wrapped in
    /// [`DocumentDownloadState`].
    async fn get_document(
        &self,
        url: Url,
    ) -> Result<(Document, Instant), (DocumentDownloadState, ExtrablattError)> {
        match crate::download::fetch_html_with_budget(
            &self.client,
            url,
            self.config.max_response_bytes,
            &self.download_budget,
            self.config.http_success_only,
        )
        .await
        {
            Ok(downloaded) => Ok((Document::from(downloaded.html.as_str()), Instant::now())),
            Err(error) => {
                let state = match &error {
                    ExtrablattError::NoHttpSuccessResponse { .. } => {
                        DocumentDownloadState::NoHttpSuccessResponse {
                            received: Instant::now(),
                        }
                    }
                    ExtrablattError::ReadDocumentError { .. } => {
                        DocumentDownloadState::DocumentReadFailure {
                            received: Instant::now(),
                        }
                    }
                    _ => DocumentDownloadState::HttpRequestFailure {
                        received: Instant::now(),
                    },
                };
                Err((state, error))
            }
        }
    }

    #[cfg(feature = "archive")]
    pub fn archive(&self) {
        unimplemented!(
            "coming as soon as reqwest 0.10 is stabilized and archiveis crate is updated to async/await"
        )
    }
}

impl<TExtractor: Extractor + Unpin> Extrablatt<TExtractor> {
    /// Stream articles using the same scheduler as the source-preserving API.
    pub fn into_stream(self) -> ArticleStream<TExtractor> {
        ArticleStream {
            inner: self.into_stream_with_source(),
        }
    }

    /// Stream articles together with their original decoded HTTP bodies.
    pub fn into_stream_with_source(self) -> SourceArticleStream<TExtractor> {
        SourceArticleStream::from_paper(self)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StreamStats {
    pub discovered_articles: usize,
    pub failed_articles: usize,
    pub failed_categories: usize,
    pub filtered_articles: usize,
    /// Accepted body bytes, including any expansion introduced by charset decoding.
    pub downloaded_bytes: usize,
}

#[derive(Debug, Default)]
struct PageRoles {
    article: bool,
    category: bool,
    article_done: bool,
    category_done: bool,
}

#[derive(Clone)]
struct CachedPage {
    url: Url,
    doc: Option<Document>,
    raw_html: Option<String>,
}

type PaperResponse = Pin<
    Box<
        dyn Future<
            Output = (
                Url,
                Result<crate::download::DownloadedHtml, ExtrablattError>,
            ),
        >,
    >,
>;

/// Source-preserving article stream with bounded requests and deduplicated pages.
#[must_use = "streams do nothing unless polled"]
pub struct SourceArticleStream<TExtractor: Extractor = DefaultExtractor> {
    paper: Extrablatt<TExtractor>,
    roles: std::collections::HashMap<Url, PageRoles>,
    cache: std::collections::HashMap<Url, CachedPage>,
    aliases: std::collections::HashMap<Url, Url>,
    failed_pages: std::collections::HashSet<Url>,
    discovered_articles: std::collections::HashSet<Url>,
    discovered_categories: std::collections::HashSet<Url>,
    emitted_articles: std::collections::HashSet<Url>,
    processed_categories: std::collections::HashSet<Url>,
    pending: std::collections::VecDeque<Url>,
    active: futures::stream::FuturesUnordered<PaperResponse>,
    ready_pages: std::collections::VecDeque<Url>,
    results: std::collections::VecDeque<Result<crate::article::ArticleWithSource, ExtrablattError>>,
    stats: StreamStats,
    stopped: bool,
}

fn normalize_url(mut url: Url) -> Url {
    url.set_fragment(None);
    url
}

fn global_limit(error: &ExtrablattError) -> bool {
    matches!(
        error,
        ExtrablattError::LimitExceeded {
            resource: "total_bytes" | "article_urls" | "category_urls",
            ..
        }
    )
}

impl<TExtractor: Extractor + Unpin> SourceArticleStream<TExtractor> {
    fn from_paper(mut paper: Extrablatt<TExtractor>) -> Self {
        let articles = std::mem::take(&mut paper.articles);
        let categories = std::mem::take(&mut paper.categories);
        let main_url = normalize_url(paper.base_url.clone());
        let mut cache = std::collections::HashMap::new();
        cache.insert(
            main_url.clone(),
            CachedPage {
                url: main_url.clone(),
                doc: if paper.main_source.is_some() {
                    None
                } else {
                    Some(paper.main_page.clone())
                },
                raw_html: paper.main_source.take(),
            },
        );
        let mut aliases = std::collections::HashMap::new();
        aliases.insert(
            normalize_url(paper.main_requested_url.clone()),
            main_url.clone(),
        );
        let stats = StreamStats {
            downloaded_bytes: paper.download_budget.used(),
            ..Default::default()
        };
        let mut stream = Self {
            paper,
            roles: Default::default(),
            cache,
            aliases,
            failed_pages: Default::default(),
            discovered_articles: Default::default(),
            discovered_categories: Default::default(),
            emitted_articles: Default::default(),
            processed_categories: Default::default(),
            pending: Default::default(),
            active: Default::default(),
            ready_pages: Default::default(),
            results: Default::default(),
            stats,
            stopped: false,
        };
        if stream.paper.config.concurrency == 0 {
            stream.stop(ExtrablattError::InvalidConfig {
                reason: "concurrency must be greater than zero",
            });
            return stream;
        }
        // Existing cached documents have no original source, so expose None without another GET.
        for (article, state) in articles {
            let url = normalize_url(article.url);
            if let DocumentDownloadState::Success { doc, .. } = state {
                stream.cache.entry(url.clone()).or_insert(CachedPage {
                    url: url.clone(),
                    doc: Some(doc),
                    raw_html: None,
                });
            } else if !state.is_not_requested() {
                stream.failed_pages.insert(url.clone());
            }
            stream.discover(url, false);
        }
        for (category, state) in categories {
            let url = normalize_url(category.url);
            if url == main_url || stream.aliases.get(&url) == Some(&main_url) {
                continue;
            }
            if let DocumentDownloadState::Success { doc, .. } = state {
                stream.cache.entry(url.clone()).or_insert(CachedPage {
                    url: url.clone(),
                    doc: Some(doc),
                    raw_html: None,
                });
            } else if !state.is_not_requested() {
                stream.failed_pages.insert(url.clone());
            }
            stream.discover(url, true);
        }
        let main_articles = stream
            .paper
            .extractor
            .article_urls(&stream.paper.main_page, Some(&main_url));
        for article in main_articles {
            stream.discover(article.url, false);
        }
        stream
    }

    pub fn stats(&self) -> &StreamStats {
        &self.stats
    }

    fn stop(&mut self, error: ExtrablattError) {
        self.stopped = true;
        self.pending.clear();
        self.active.clear();
        self.ready_pages.clear();
        self.results.push_back(Err(error));
    }

    fn discover(&mut self, url: Url, category: bool) {
        if self.stopped || !matches!(url.scheme(), "http" | "https") {
            return;
        }
        let url = normalize_url(url);
        let known = if category {
            &mut self.discovered_categories
        } else {
            &mut self.discovered_articles
        };
        if known.contains(&url) {
            return;
        }
        let limit = if category {
            self.paper.config.max_category_urls
        } else {
            self.paper.config.max_article_urls
        };
        if let Some(limit) = limit {
            if known.len() >= limit {
                self.stop(ExtrablattError::LimitExceeded {
                    resource: if category {
                        "category_urls"
                    } else {
                        "article_urls"
                    },
                    limit,
                });
                return;
            }
        }
        known.insert(url.clone());
        self.stats.discovered_articles = self.discovered_articles.len();
        let first_role = !self.roles.contains_key(&url);
        let roles = self.roles.entry(url.clone()).or_default();
        if category {
            roles.category = true;
        } else {
            roles.article = true;
        }
        if self.failed_pages.contains(&url) {
            self.fail_page(&url, ExtrablattError::PreviouslyFailedPage);
        } else {
            let key = self.aliases.get(&url).unwrap_or(&url);
            if self.cache.contains_key(key) {
                self.ready_pages.push_back(url);
            } else if first_role {
                self.pending.push_back(url);
            }
        }
    }

    fn fail_page(&mut self, url: &Url, error: ExtrablattError) {
        if global_limit(&error) {
            self.stop(error);
            return;
        }
        self.failed_pages.insert(url.clone());
        let roles = self.roles.get_mut(url).expect("scheduled page");
        let article = roles.article && !roles.article_done;
        let category = roles.category && !roles.category_done;
        if article {
            self.stats.failed_articles += 1;
            roles.article_done = true;
        }
        if category {
            self.stats.failed_categories += 1;
            roles.category_done = true;
        }
        self.results.push_back(Err(if category && !article {
            ExtrablattError::CategoryFailure {
                error: Box::new(error),
            }
        } else {
            error
        }));
    }

    fn process_page(&mut self, requested: Url) {
        let key = self.aliases.get(&requested).unwrap_or(&requested).clone();
        let Some(page) = self.cache.get(&key).cloned() else {
            return;
        };
        let doc = page
            .doc
            .clone()
            .unwrap_or_else(|| Document::from(page.raw_html.as_deref().unwrap_or("")));
        let roles = self.roles.get_mut(&requested).expect("discovered page");
        if roles.category && !roles.category_done {
            roles.category_done = true;
            if self.processed_categories.insert(page.url.clone()) {
                let urls = self.paper.extractor.article_urls(&doc, Some(&page.url));
                for article in urls {
                    self.discover(article.url, false);
                }
            }
        }
        if self.stopped {
            return;
        }
        let roles = self.roles.get_mut(&requested).expect("discovered page");
        if !roles.article || roles.article_done {
            return;
        }
        roles.article_done = true;
        if !self.emitted_articles.insert(page.url.clone()) {
            return;
        }
        let content = self
            .paper
            .extractor
            .article_content(&doc, Some(&page.url), Some(self.paper.language.clone()))
            .into_owned();
        let language = content
            .language
            .clone()
            .unwrap_or_else(|| self.paper.language.clone());
        if content
            .text
            .as_deref()
            .is_none_or(|text| text.trim().is_empty())
        {
            self.stats.failed_articles += 1;
            self.results.push_back(Err(ExtrablattError::EmptyArticle));
        } else if !self.paper.config.is_complete(&content) {
            self.stats.filtered_articles += 1;
            self.results
                .push_back(Err(ExtrablattError::IncompleteArticle {
                    article: Box::new(PureArticle {
                        url: page.url,
                        content,
                        language,
                    }),
                }));
        } else {
            self.results
                .push_back(Ok(crate::article::ArticleWithSource {
                    article: Article {
                        url: page.url,
                        doc,
                        content,
                        language,
                    },
                    raw_html: page.raw_html,
                }));
        }
    }

    fn start_requests(&mut self) {
        while !self.stopped && self.active.len() < self.paper.config.concurrency {
            let Some(requested) = self.pending.pop_front() else {
                break;
            };
            let key = self.aliases.get(&requested).unwrap_or(&requested);
            if self.cache.contains_key(key) {
                self.ready_pages.push_back(requested);
                continue;
            }
            let client = self.paper.client.clone();
            let limit = self.paper.config.max_response_bytes;
            let budget = self.paper.download_budget.clone();
            let success_only = self.paper.config.http_success_only;
            self.active.push(Box::pin(async move {
                let result = crate::download::fetch_html_with_budget(
                    &client,
                    requested.clone(),
                    limit,
                    &budget,
                    success_only,
                )
                .await;
                (requested, result)
            }));
        }
    }
}

impl<TExtractor: Extractor + Unpin> Stream for SourceArticleStream<TExtractor> {
    type Item = Result<crate::article::ArticleWithSource, ExtrablattError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        loop {
            self.stats.downloaded_bytes = self.paper.download_budget.used();
            if let Some(result) = self.results.pop_front() {
                return Poll::Ready(Some(result));
            }
            if self.stopped {
                return Poll::Ready(None);
            }
            if let Some(url) = self.ready_pages.pop_front() {
                self.process_page(url);
                continue;
            }
            self.start_requests();
            if !self.ready_pages.is_empty() {
                continue;
            }
            match Pin::new(&mut self.active).poll_next(cx) {
                Poll::Ready(Some((requested, result))) => {
                    match result {
                        Ok(downloaded) => {
                            let url = normalize_url(downloaded.url);
                            self.aliases.insert(requested.clone(), url.clone());
                            self.cache.entry(url.clone()).or_insert_with(|| CachedPage {
                                doc: None,
                                raw_html: Some(downloaded.html),
                                url,
                            });
                            self.ready_pages.push_back(requested);
                        }
                        Err(error) => self.fail_page(&requested, error),
                    }
                    // Poll newly discovered category work before yielding Pending.
                    continue;
                }
                Poll::Ready(None) if self.pending.is_empty() => return Poll::Ready(None),
                Poll::Ready(None) => continue,
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Compatibility adapter for callers that only need extracted articles.
#[must_use = "streams do nothing unless polled"]
pub struct ArticleStream<TExtractor: Extractor = DefaultExtractor> {
    inner: SourceArticleStream<TExtractor>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ArticleStream<DefaultExtractor> {
    pub async fn new<T: IntoUrl>(url: T) -> Result<Self, ExtrablattError> {
        Self::new_with_extractor(url, DefaultExtractor).await
    }
}

impl<TExtractor: Extractor + Unpin> ArticleStream<TExtractor> {
    pub async fn new_with_extractor<T: IntoUrl>(
        url: T,
        extractor: TExtractor,
    ) -> Result<Self, ExtrablattError> {
        Ok(ExtrablattBuilder::new(url)?
            .categories(false)
            .build_with_extractor(extractor)
            .await?
            .into_stream())
    }

    pub fn stats(&self) -> &StreamStats {
        self.inner.stats()
    }
}

impl<TExtractor: Extractor + Unpin> Stream for ArticleStream<TExtractor> {
    type Item = Result<Article, ExtrablattError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner)
            .poll_next(cx)
            .map(|item| item.map(|result| result.map(|sourced| sourced.article)))
    }
}

#[derive(Debug)]
pub struct ExtrablattBuilder {
    base_url: Option<Url>,
    config: Option<Config>,
    language: Option<Language>,
    headers: Option<HeaderMap>,
    /// Whether to extract and store the categories of the news paper's main
    /// page.
    categories: bool,
    /// Optional proxy URL for HTTP requests.
    /// Format: "http://proxy:port" or "https://proxy:port" or "socks5://proxy:port"
    proxy: Option<String>,
    /// If true, disable automatic system proxy detection from environment variables.
    /// When set, HTTP_PROXY/HTTPS_PROXY/ALL_PROXY env vars will be ignored.
    no_system_proxy: bool,
}

impl ExtrablattBuilder {
    pub fn new<T: IntoUrl>(base_url: T) -> Result<Self, ExtrablattError> {
        Ok(Self {
            base_url: Some(
                base_url
                    .into_url()
                    .map_err(|e| ExtrablattError::UrlParseError { error: e })?,
            ),
            config: None,
            language: None,
            headers: None,
            categories: true,
            proxy: None,
            no_system_proxy: false,
        })
    }

    pub fn language(mut self, language: Language) -> Self {
        self.language = Some(language);
        self
    }

    pub fn config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    pub fn headers(mut self, headers: HeaderMap) -> Self {
        self.headers = Some(headers);
        self
    }

    pub fn categories(mut self, categories: bool) -> Self {
        self.categories = categories;
        self
    }

    /// Set a proxy for all HTTP requests.
    ///
    /// The proxy URL should be in the format:
    /// - `http://proxy:port` for HTTP proxy
    /// - `https://proxy:port` for HTTPS proxy
    /// - `socks5://proxy:port` for SOCKS5 proxy
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use extrablatt_v2::Extrablatt;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let site = Extrablatt::builder("https://example.com")?
    ///         .proxy("http://127.0.0.1:8080")
    ///         .build()
    ///         .await?;
    /// #   Ok(())
    /// # }
    /// ```
    pub fn proxy<T: Into<String>>(mut self, proxy: T) -> Self {
        self.proxy = Some(proxy.into());
        self
    }

    /// Disable automatic system proxy detection from environment variables.
    ///
    /// When called, the HTTP_PROXY, HTTPS_PROXY, ALL_PROXY, and NO_PROXY
    /// environment variables will be ignored. Use this when you want to
    /// ensure no proxy is used unless explicitly set via `.proxy()`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use extrablatt_v2::Extrablatt;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     // This will NOT use any system proxy, even if HTTP_PROXY is set
    ///     let site = Extrablatt::builder("https://example.com")?
    ///         .no_system_proxy()
    ///         .build()
    ///         .await?;
    /// #   Ok(())
    /// # }
    /// ```
    pub fn no_system_proxy(mut self) -> Self {
        self.no_system_proxy = true;
        self
    }

    /// Create a new builder with a specific extractor.
    pub async fn build_with_extractor<TExtractor: Extractor>(
        self,
        extractor: TExtractor,
    ) -> Result<Extrablatt<TExtractor>, ExtrablattError> {
        let base_url = self.base_url.ok_or(ExtrablattError::UrlNotInitialized)?;
        if base_url.cannot_be_a_base() {
            return Err(ExtrablattError::BaseUrlInvalid { url: base_url });
        }
        let config = self.config.unwrap_or_default();
        if config.concurrency == 0 {
            return Err(ExtrablattError::InvalidConfig {
                reason: "concurrency must be greater than zero",
            });
        }
        if config.request_timeout.is_zero() {
            return Err(ExtrablattError::InvalidConfig {
                reason: "request timeout must be greater than zero",
            });
        }
        #[cfg(target_arch = "wasm32")]
        let client = {
            Client::builder()
                .build()
                .map_err(ExtrablattError::Reqwest)?
        };
        #[cfg(not(target_arch = "wasm32"))]
        let client = {
            let mut headers = self.headers.unwrap_or_else(|| HeaderMap::with_capacity(1));
            if !headers.contains_key(USER_AGENT) {
                headers.insert(
                    USER_AGENT,
                    config
                        .user_agent
                        .parse()
                        .map_err(|_| ExtrablattError::UserAgentParseError)?,
                );
            }
            let mut client_builder = Client::builder()
                .default_headers(headers)
                .timeout(config.request_timeout);

            // Configure proxy if provided, or disable system proxy if requested
            if let Some(ref proxy_url) = self.proxy {
                let proxy = reqwest::Proxy::all(proxy_url).map_err(ExtrablattError::Reqwest)?;
                client_builder = client_builder.proxy(proxy);
            } else if self.no_system_proxy {
                // Disable reading proxy settings from environment variables
                client_builder = client_builder.no_proxy();
            }

            client_builder.build().map_err(ExtrablattError::Reqwest)?
        };
        let download_budget = crate::download::DownloadBudget::new(config.max_total_bytes);
        let main_requested_url = base_url.clone();
        let downloaded = crate::download::fetch_html_with_budget(
            &client,
            base_url,
            config.max_response_bytes,
            &download_budget,
            config.http_success_only,
        )
        .await?;
        let main_page = Document::from(downloaded.html.as_str());
        let main_source = Some(downloaded.html);
        let base_url = normalize_url(downloaded.url);
        let mut paper = Extrablatt {
            client,
            language: self.language.unwrap_or_default(),
            main_page,
            main_source,
            main_requested_url,
            download_budget,
            base_url,
            extractor,
            categories: Default::default(),
            articles: FnvHashMap::with_capacity_and_hasher(
                config.max_doc_cache,
                Default::default(),
            ),
            config,
        };
        if self.categories {
            paper.insert_new_categories();
        }
        Ok(paper)
    }
    pub async fn build(self) -> Result<Extrablatt, ExtrablattError> {
        self.build_with_extractor(Default::default()).await
    }
}

#[derive(Debug)]
pub enum DocumentDownloadState {
    /// No request sent yet.
    NotRequested,
    /// Received a success response at `received` and successfully parsed the
    /// html document.
    Success {
        /// Timestamp the response was received.
        received: Instant,
        /// The parsed html body.
        doc: Document,
    },
    NoHttpSuccessResponse {
        /// Timestamp the response was received.
        received: Instant,
    },
    /// Received an error response.
    HttpRequestFailure {
        /// Timestamp the response was received.
        received: Instant,
    },
    /// Received a success response at `received` but failed to parse the `body`
    /// into a [`select::document::Document`]
    DocumentReadFailure {
        /// Timestamp the response was received.
        received: Instant,
    },
}

impl DocumentDownloadState {
    async fn read_response(
        response: Response,
    ) -> Result<(Document, Instant), (DocumentDownloadState, ExtrablattError)> {
        match response.bytes().await {
            Ok(body) => {
                if let Ok(doc) = Document::from_read(&*body) {
                    Ok((doc, Instant::now()))
                } else {
                    Err((
                        DocumentDownloadState::DocumentReadFailure {
                            received: Instant::now(),
                        },
                        ExtrablattError::ReadDocumentError { body },
                    ))
                }
            }
            Err(error) => Err((
                DocumentDownloadState::HttpRequestFailure {
                    received: Instant::now(),
                },
                ExtrablattError::HttpRequestFailure { error },
            )),
        }
    }

    /// If the error is due to an non 2xx response, try to read it into an
    /// [`select::document::Document`] anyway.
    async fn advance_non_http_success(
        err: ExtrablattError,
    ) -> Result<(Document, Instant), ExtrablattError> {
        if let ExtrablattError::NoHttpSuccessResponse { response } = err {
            match DocumentDownloadState::read_response(response).await {
                Ok((doc, received)) => Ok((doc, received)),
                Err((_, err)) => Err(err),
            }
        } else {
            Err(err)
        }
    }

    /// The actual document if the request was successful.
    pub fn success_document(&self) -> Option<&Document> {
        match self {
            DocumentDownloadState::Success { doc, .. } => Some(doc),
            _ => None,
        }
    }

    pub fn is_http_failure(&self) -> bool {
        matches!(self, DocumentDownloadState::HttpRequestFailure { .. })
    }

    pub fn is_no_http_success_response(&self) -> bool {
        matches!(self, DocumentDownloadState::NoHttpSuccessResponse { .. })
    }

    pub fn is_doc_parsing_failure(&self) -> bool {
        matches!(self, DocumentDownloadState::DocumentReadFailure { .. })
    }

    pub fn is_not_requested(&self) -> bool {
        matches!(self, DocumentDownloadState::NotRequested { .. })
    }

    pub fn is_success(&self) -> bool {
        matches!(self, DocumentDownloadState::Success { .. })
    }
}

impl Default for DocumentDownloadState {
    fn default() -> Self {
        DocumentDownloadState::NotRequested
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Minimum number of word tokens in the text.
    min_word_count: Option<usize>,
    /// Maximum number of word tokens in the text.
    max_word_count: Option<usize>,
    /// Number of sentence tokens.
    min_sentence_count: Option<usize>,
    /// Min number of chars for the text's title.
    min_title_len: Option<usize>,
    /// Max number of chars for the text's title.
    max_title_len: Option<usize>,
    /// Min number of chars for the text.
    min_text_len: Option<usize>,
    /// Max number of chars for the text.
    max_text_len: Option<usize>,
    /// Min number of keywords for the text.
    min_keywords: Option<usize>,
    /// Max number of keywords for the text.
    max_keywords: Option<usize>,
    /// Min number of Authors.
    min_authors: Option<usize>,
    /// Max number of Authors.
    max_authors: Option<usize>,
    /// Max. number of urls to cache for a news source.
    max_doc_cache: usize,
    /// Whether to also capture non 2XX responses.
    http_success_only: bool,
    /// The user-agent used for requests.
    user_agent: String,
    /// Timeout for requests.
    request_timeout: Duration,
    /// Number of concurrent requests for downloading articles/categories.
    concurrency: usize,
    max_response_bytes: Option<usize>,
    max_total_bytes: Option<usize>,
    max_article_urls: Option<usize>,
    max_category_urls: Option<usize>,
}

impl Config {
    /// Default timeout for requests made inside `extrablatt`.
    pub const DEFAULT_REQUEST_TIMEOUT_SEC: u64 = 30;

    /// Default number of concurrent requests.
    pub const DEFAULT_CONCURRENCY: usize = 10;

    /// Default user agent for `extrablatt`.
    #[inline]
    pub(crate) fn user_agent() -> String {
        format!("extrablatt/{}", env!("CARGO_PKG_VERSION"))
    }

    /// Convenience method to create a [`ConfigBuilder`]
    #[inline]
    pub fn builder() -> ConfigBuilder {
        ConfigBuilder::default()
    }

    /// Number of concurrent requests for downloading articles/categories.
    #[inline]
    pub fn concurrency(&self) -> usize {
        self.concurrency
    }

    /// Checks that the article fulfills the configured restrictions.
    pub fn is_complete<'a>(&self, article: &ArticleContent<'a>) -> bool {
        macro_rules! range_check {
            ($($el:expr => ($min:expr ; $max:expr)),*) => {
                $(
                 if let Some(min) = $min {
                    if let Some(txt) = &$el {
                        if min > txt.len() {
                            return false
                        }
                    } else {
                        return false
                    }
                 }

                if let Some(max) = $max {
                    if let Some(txt) = &$el {
                        if max < txt.len() {
                            return false
                        }
                    }
                }

                )*
            };
        }

        let mut words = None;
        if let Some(min_word_count) = self.min_word_count {
            if let Some(txt) = &article.text {
                let words_ctn = ArticleTextNodeExtractor::words(txt.as_ref()).count();
                if min_word_count > words_ctn {
                    return false;
                }
                words = Some(words_ctn);
            } else {
                return false;
            }
        }

        if let Some(max_word_count) = self.max_word_count {
            if let Some(txt) = &article.text {
                let words_ctn =
                    words.unwrap_or_else(|| ArticleTextNodeExtractor::words(txt.as_ref()).count());
                if max_word_count < words_ctn {
                    return false;
                }
            }
        }

        range_check!(
            article.text => (self.min_text_len ; self.max_text_len),
            article.title => (self.min_title_len ; self.max_title_len)
        );

        if let Some(min_keywords) = self.min_keywords {
            if min_keywords > article.keywords.len() {
                return false;
            }
        }

        if let Some(max_keywords) = self.max_keywords {
            if max_keywords < article.keywords.len() {
                return false;
            }
        }

        if let Some(min_authors) = self.min_authors {
            if min_authors > article.authors.len() {
                return false;
            }
        }

        if let Some(max_authors) = self.max_authors {
            if max_authors < article.authors.len() {
                return false;
            }
        }

        true
    }
}

impl Default for Config {
    fn default() -> Self {
        Config::builder().build()
    }
}

#[derive(Debug, Default)]
pub struct ConfigBuilder {
    /// Minimum number of word tokens in the text.
    min_word_count: Option<usize>,
    /// Maximum number of word tokens in the text.
    max_word_count: Option<usize>,
    /// Number of sentence tokens.
    min_sentence_count: Option<usize>,
    /// Min number of chars for the text's title.
    min_title_len: Option<usize>,
    /// Max number of chars for the text's title.
    max_title_len: Option<usize>,
    /// Min number of chars for the text.
    min_text_len: Option<usize>,
    /// Max number of chars for the text.
    max_text_len: Option<usize>,
    /// Min number of keywords for the text.
    min_keywords: Option<usize>,
    /// Max number of keywords for the text.
    max_keywords: Option<usize>,
    /// Min number of Authors.
    min_authors: Option<usize>,
    /// Max number of Authors.
    max_authors: Option<usize>,
    /// Max. number of urls to cache for each news source.
    max_doc_cache: Option<usize>,
    /// Whether to also capture non 2XX responses.
    http_success_only: Option<bool>,
    /// The user-agent used for requests.
    user_agent: Option<String>,
    /// Timeout for requests.
    request_timeout: Option<Duration>,
    /// Number of concurrent requests for downloading articles/categories.
    concurrency: Option<usize>,
    max_response_bytes: Option<usize>,
    max_total_bytes: Option<usize>,
    max_article_urls: Option<usize>,
    max_category_urls: Option<usize>,
}

impl ConfigBuilder {
    pub fn min_word_count(mut self, min_word_count: usize) -> Self {
        self.min_word_count = Some(min_word_count);
        self
    }

    pub fn max_word_count(mut self, max_word_count: usize) -> Self {
        self.max_word_count = Some(max_word_count);
        self
    }

    pub fn min_sentence_count(mut self, min_sentence_count: usize) -> Self {
        self.min_sentence_count = Some(min_sentence_count);
        self
    }

    pub fn min_title_len(mut self, min_title_len: usize) -> Self {
        self.min_title_len = Some(min_title_len);
        self
    }

    pub fn max_title_len(mut self, max_title_len: usize) -> Self {
        self.max_title_len = Some(max_title_len);
        self
    }

    pub fn min_text_len(mut self, min_text_len: usize) -> Self {
        self.min_text_len = Some(min_text_len);
        self
    }

    pub fn max_text_len(mut self, max_text_len: usize) -> Self {
        self.max_text_len = Some(max_text_len);
        self
    }

    pub fn min_keywords(mut self, min_keywords: usize) -> Self {
        self.min_keywords = Some(min_keywords);
        self
    }

    pub fn max_keywords(mut self, max_keywords: usize) -> Self {
        self.max_keywords = Some(max_keywords);
        self
    }

    pub fn min_authors(mut self, min_authors: usize) -> Self {
        self.min_authors = Some(min_authors);
        self
    }

    pub fn max_authors(mut self, max_authors: usize) -> Self {
        self.max_authors = Some(max_authors);
        self
    }

    pub fn max_doc_cache(mut self, max_doc_cache: usize) -> Self {
        self.max_doc_cache = Some(max_doc_cache);
        self
    }

    pub fn http_success_only(mut self, keep_article_html: bool) -> Self {
        self.http_success_only = Some(keep_article_html);
        self
    }

    pub fn user_agent<T: ToString>(mut self, user_agent: T) -> Self {
        self.user_agent = Some(user_agent.to_string());
        self
    }

    pub fn request_timeout(mut self, request_timeout: Duration) -> Self {
        self.request_timeout = Some(request_timeout);
        self
    }

    /// Set the number of concurrent requests for downloading articles/categories.
    ///
    /// Default is [`Config::DEFAULT_CONCURRENCY`] (10).
    pub fn concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = Some(concurrency);
        self
    }

    /// Bound each downloaded body and its decoded UTF-8 representation.
    /// Native clients check incoming chunks; wasm clients check after browser buffering.
    pub fn max_response_bytes(mut self, limit: usize) -> Self {
        self.max_response_bytes = Some(limit);
        self
    }
    /// Bound accepted body bytes across the lifetime of this newspaper instance.
    /// Includes the homepage, refreshes, categories, articles and charset expansion.
    pub fn max_total_bytes(mut self, limit: usize) -> Self {
        self.max_total_bytes = Some(limit);
        self
    }
    /// Bound distinct normalized article URLs discovered by the streaming APIs.
    /// Legacy bulk download methods do not apply this URL-count limit.
    pub fn max_article_urls(mut self, limit: usize) -> Self {
        self.max_article_urls = Some(limit);
        self
    }
    /// Bound distinct normalized category URLs processed by the streaming APIs.
    /// Legacy bulk download methods do not apply this URL-count limit.
    pub fn max_category_urls(mut self, limit: usize) -> Self {
        self.max_category_urls = Some(limit);
        self
    }

    pub fn build(self) -> Config {
        Config {
            min_word_count: self.min_word_count,
            max_word_count: self.max_word_count,
            min_sentence_count: self.min_sentence_count,
            min_title_len: self.min_title_len,
            max_title_len: self.max_title_len,
            min_text_len: self.min_text_len,
            max_text_len: self.max_text_len,
            min_keywords: self.min_keywords,
            max_keywords: self.max_keywords,
            min_authors: self.min_authors,
            max_authors: self.max_authors,
            max_doc_cache: self.max_doc_cache.unwrap_or(2_0000),
            http_success_only: self.http_success_only.unwrap_or(true),
            user_agent: self.user_agent.unwrap_or_else(Config::user_agent),
            request_timeout: self
                .request_timeout
                .unwrap_or_else(|| Duration::from_secs(Config::DEFAULT_REQUEST_TIMEOUT_SEC)),
            concurrency: self.concurrency.unwrap_or(Config::DEFAULT_CONCURRENCY),
            max_response_bytes: self.max_response_bytes,
            max_total_bytes: self.max_total_bytes,
            max_article_urls: self.max_article_urls,
            max_category_urls: self.max_category_urls,
        }
    }

    /// Pre sets some restrictions regarding the article content:
    ///     * Word Count >= 300
    ///     * Sentences >= 7
    ///     * Text Length >= 100_000
    pub fn with_restrictions() -> Self {
        Self {
            min_word_count: Some(300),
            max_word_count: None,
            min_sentence_count: Some(7),
            min_title_len: None,
            max_title_len: Some(200),
            min_text_len: None,
            max_text_len: Some(100_000),
            min_keywords: None,
            max_keywords: None,
            min_authors: None,
            max_authors: None,
            max_doc_cache: Some(2_0000),
            http_success_only: None,
            user_agent: None,
            request_timeout: None,
            concurrency: None,
            max_response_bytes: None,
            max_total_bytes: None,
            max_article_urls: None,
            max_category_urls: None,
        }
    }
}

/// Iterator over the downloaded articles.
pub struct ArticleDownloadIter<'a, T: Extractor> {
    /// Each found url for an article paired with the result of it's request.
    inner: std::collections::hash_map::Iter<'a, ArticleUrl, DocumentDownloadState>,
    /// The `Extractor` used to get the article's content.
    extractor: &'a T,
    /// Language of the news source.
    language: Language,
    /// Base url of the news source.
    base_url: &'a Url,
}

impl<'a, T: Extractor> ArticleDownloadIter<'a, T> {
    /// All successfully retrieved Articles.
    pub fn successes(self) -> impl Iterator<Item = (&'a ArticleUrl, ArticleContent<'a>)> + 'a {
        let extractor = self.extractor;
        let language = self.language;
        let base_url = self.base_url;
        self.inner.filter_map(move |(url, doc)| {
            if let DocumentDownloadState::Success { doc, .. } = doc {
                Some((
                    url,
                    extractor.article_content(doc, Some(base_url), Some(language.clone())),
                ))
            } else {
                None
            }
        })
    }
}

impl<'a, T: Extractor> Deref for ArticleDownloadIter<'a, T> {
    type Target = std::collections::hash_map::Iter<'a, ArticleUrl, DocumentDownloadState>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'a, T: Extractor> DerefMut for ArticleDownloadIter<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

#[cfg(test)]
#[path = "stream_regression_tests.rs"]
mod stream_regression_tests;

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "source_stream_tests.rs"]
mod source_stream_tests;
