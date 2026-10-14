use std::borrow::{Borrow, Cow};
use std::hash::{Hash, Hasher};
use std::time::Duration;

use std::fmt;
#[derive(Debug)]
pub enum ArticleError {
    UrlNotInitialized,
    UserAgentParseError,
    UnsuccessfulRequest(String),
    DocumentReadError(String),
    Reqwest(reqwest::Error),
    HttpStatus(reqwest::StatusCode),
    LimitExceeded {
        resource: &'static str,
        limit: usize,
    },
    Download(Box<crate::ExtrablattError>),
    Other(String),
}

impl fmt::Display for ArticleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArticleError::UrlNotInitialized => write!(f, "Url of the article must be initialized."),
            ArticleError::UserAgentParseError => write!(f, "Failed to parse user agent header."),
            ArticleError::UnsuccessfulRequest(url) => write!(f, "Unsuccessful request to {}", url),
            ArticleError::DocumentReadError(url) => {
                write!(f, "Failed to read {} html as document.", url)
            }
            ArticleError::Reqwest(e) => write!(f, "Reqwest error: {}", e),
            ArticleError::HttpStatus(status) => write!(f, "Article HTTP request failed: {status}"),
            ArticleError::LimitExceeded { resource, limit } => {
                write!(f, "{resource} exceeds limit {limit}")
            }
            ArticleError::Download(error) => write!(f, "Article download failed: {error}"),
            ArticleError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for ArticleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ArticleError::Reqwest(e) => Some(e),
            ArticleError::Download(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for ArticleError {
    fn from(e: reqwest::Error) -> Self {
        ArticleError::Reqwest(e)
    }
}

impl From<crate::ExtrablattError> for ArticleError {
    fn from(error: crate::ExtrablattError) -> Self {
        match error {
            crate::ExtrablattError::HttpRequestFailure { error }
            | crate::ExtrablattError::Reqwest(error)
            | crate::ExtrablattError::UrlParseError { error } => Self::Reqwest(error),
            crate::ExtrablattError::NoHttpSuccessResponse { response } => {
                Self::HttpStatus(response.status())
            }
            crate::ExtrablattError::LimitExceeded { resource, limit } => {
                Self::LimitExceeded { resource, limit }
            }
            other => Self::Download(Box::new(other)),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
use reqwest::header::{HeaderMap, USER_AGENT};
use reqwest::{Client, IntoUrl, Url};
use select::document::Document;
#[cfg(feature = "serde0")]
use serde::{Deserialize, Serialize};

use crate::date::ArticleDate;
#[cfg(not(target_arch = "wasm32"))]
use crate::extrablatt::Config;
use crate::extract::{DefaultExtractor, Extractor};
use crate::language::Language;

/// Extension for documents that are considered valid sources for articles.
pub const ALLOWED_FILE_EXT: [&str; 12] = [
    "html", "htm", "md", "rst", "aspx", "jsp", "rhtml", "cgi", "xhtml", "jhtml", "asp", "shtml",
];

/// Segments in an url path that usually point to valid articles.
pub const GOOD_SEGMENTS: [&str; 15] = [
    "story",
    "article",
    "feature",
    "featured",
    "slides",
    "slideshow",
    "gallery",
    "news",
    "video",
    "media",
    "v",
    "radio",
    "press",
    "graphics",
    "investigations",
];

/// Segments in an url path that usually point to invalid articles.
pub const BAD_SEGMENTS: [&str; 17] = [
    "careers",
    "contact",
    "about",
    "faq",
    "terms",
    "privacy",
    "advert",
    "preferences",
    "feedback",
    "info",
    "browse",
    "howto",
    "account",
    "subscribe",
    "donate",
    "shop",
    "admin",
];

/// Domain names that are treated as bad sources for articles.
pub const BAD_DOMAINS: [&str; 4] = ["amazon", "doubleclick", "twitter", "outbrain"];

/// An identified url to an article and it's title.
#[derive(Debug, Clone)]
pub struct ArticleUrl {
    /// The url of the article.
    pub url: Url,
    /// The title of the article.
    pub title: Option<String>,
}

impl ArticleUrl {
    pub fn new(url: Url) -> Self {
        Self { url, title: None }
    }

    pub fn new_with_title<T: ToString>(url: Url, title: Option<T>) -> Self {
        Self {
            url,
            title: title.map(|s| s.to_string()),
        }
    }

    /// If the article is related heavily to media: gallery, video, big
    /// pictures, etc
    pub fn is_media_news(&self) -> bool {
        if let Some(segments) = self.url.path_segments() {
            let media_segemnts = &[
                "video",
                "slide",
                "gallery",
                "powerpoint",
                "fashion",
                "glamour",
                "cloth",
                "graphics",
            ];
            for segment in segments.filter(|s| s.len() < 11) {
                if media_segemnts.contains(&segment.to_lowercase().as_str()) {
                    return true;
                }
            }
        }
        false
    }
}

impl PartialEq for ArticleUrl {
    fn eq(&self, other: &Self) -> bool {
        self.url.eq(&other.url)
    }
}

impl Hash for ArticleUrl {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.url.hash(state)
    }
}

impl Eq for ArticleUrl {}

impl Borrow<Url> for ArticleUrl {
    fn borrow(&self) -> &Url {
        &self.url
    }
}

/// A full representation of a news article.
///
/// # Example
///
/// Download a single article directly
///
/// ```no_run
/// # use extrablatt_v2::Article;
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let article = Article::builder("http://example.com/")?.get().await?;
/// #   Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Article {
    /// The url of the article.
    pub url: Url,
    /// The parsed response html `Document`.
    pub doc: Document,
    /// The extracted content of the article.
    pub content: ArticleContent<'static>,
    /// The expected language of the article.
    pub language: Language,
}

impl Article {
    /// Extract the article directly from the doc using the [`DefaultExtractor`]
    pub fn new<U: IntoUrl, T: AsRef<str>>(url: U, doc: T) -> Result<Article, ArticleError> {
        Self::with_extractor(url, doc, &DefaultExtractor::default())
    }

    pub fn with_extractor<U, T, TExtract>(
        url: U,
        doc: T,
        extractor: &TExtract,
    ) -> Result<Article, ArticleError>
    where
        U: IntoUrl,
        T: AsRef<str>,
        TExtract: Extractor,
    {
        Self::with_extractor_and_lang(url, doc, extractor, Default::default())
    }

    /// Extract the article directly from the doc using the provided `extractor`
    /// and `lang`
    pub fn with_extractor_and_lang<U, T, TExtract>(
        url: U,
        doc: T,
        extractor: &TExtract,
        language: Language,
    ) -> Result<Article, ArticleError>
    where
        U: IntoUrl,
        T: AsRef<str>,
        TExtract: Extractor,
    {
        let url = url.into_url()?;
        let doc = Document::from(doc.as_ref());
        let content = extractor
            .article_content(
                &doc,
                extractor.base_url(&doc).as_ref(),
                Some(language.clone()),
            )
            .into_owned();

        // Use the detected language from content if available, otherwise use the provided language
        let final_language = content.language.clone().unwrap_or(language);

        Ok(Article {
            url,
            doc,
            content,
            language: final_language,
        })
    }

    /// Retrieves the [`ArticleContent`] from the `url`
    ///
    /// Convenience method for:
    ///
    /// ```no_run
    ///  # use extrablatt_v2::Article;
    ///  # #[tokio::main]
    ///  # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///        let content = Article::builder("http://example.com")?.get().await?.content;
    ///  #     Ok(())
    ///  # }
    /// ```
    pub async fn content<T: IntoUrl>(url: T) -> Result<ArticleContent<'static>, ArticleError> {
        let article = Self::get(url).await?;
        Ok(article.content)
    }

    /// Get the [`Article`] for the `url`
    pub async fn get<T: IntoUrl>(url: T) -> Result<Article, ArticleError> {
        Self::builder(url)?.get().await
    }

    /// Get the [`Article`] for the `url` using a specific `Extractor`
    pub async fn get_with_extractor<T: IntoUrl, TExtract: Extractor>(
        url: T,
        extractor: &TExtract,
    ) -> Result<Article, ArticleError> {
        Self::builder(url)?.get_with_extractor(extractor).await
    }

    /// Convenience method for creating a new [`ArticleBuilder`]
    ///
    /// Same as calling [`ArticleBuilder::new`]
    pub fn builder<T: IntoUrl>(url: T) -> Result<ArticleBuilder, ArticleError> {
        ArticleBuilder::new(url)
    }

    /// Serialize the selected article node while preserving its markup.
    ///
    /// The fragment is not sanitized and is distinct from the original response source.
    pub fn article_html(&self) -> Option<String> {
        crate::extract_node::article_node(&self.doc, self.language.clone()).map(|node| node.html())
    }

    /// Format the publication date, preserving a source timezone when available.
    ///
    /// The existing date type remains unchanged. Naive dates stay timezone-free.
    pub fn publication_date_iso8601(&self) -> Option<String> {
        crate::publication_date::publication_date_iso8601(self)
    }

    /// Drops the article's html [`select::document::Document`].
    pub fn drop_document(self) -> PureArticle {
        PureArticle {
            url: self.url,
            content: self.content,
            language: self.language,
        }
    }
}

/// An article with its original supplied HTML or decoded HTTP response body.
///
/// `None` means source bytes were not retained, for example when reading a previously cached DOM. Serializing a DOM does not reconstruct the original HTML source.
#[derive(Debug)]
pub struct ArticleWithSource {
    pub article: Article,
    pub raw_html: Option<String>,
}

impl ArticleWithSource {
    pub fn new<U: IntoUrl, T: AsRef<str>>(url: U, html: T) -> Result<Self, ArticleError> {
        Self::with_extractor(url, html, &DefaultExtractor::default())
    }

    pub fn with_extractor<U, T, TExtract>(
        url: U,
        html: T,
        extractor: &TExtract,
    ) -> Result<Self, ArticleError>
    where
        U: IntoUrl,
        T: AsRef<str>,
        TExtract: Extractor,
    {
        Self::with_extractor_and_lang(url, html, extractor, Language::default())
    }

    pub fn with_extractor_and_lang<U, T, TExtract>(
        url: U,
        html: T,
        extractor: &TExtract,
        language: Language,
    ) -> Result<Self, ArticleError>
    where
        U: IntoUrl,
        T: AsRef<str>,
        TExtract: Extractor,
    {
        let raw_html = html.as_ref().to_owned();
        let article = Article::with_extractor_and_lang(url, &raw_html, extractor, language)?;
        Ok(Self {
            article,
            raw_html: Some(raw_html),
        })
    }
}

/// An [`crate::Article`] without the [`select::document::Document`], mainly to
/// use serde.
#[derive(Debug)]
#[cfg_attr(feature = "serde0", derive(Serialize, Deserialize))]
pub struct PureArticle {
    /// The url of the article.
    pub url: Url,
    /// The extracted content of the article.
    pub content: ArticleContent<'static>,
    /// The expected language of the article.
    pub language: Language,
}

pub struct ArticleBuilder {
    url: Option<Url>,
    timeout: Option<Duration>,
    language: Option<Language>,
    browser_user_agent: Option<String>,
    max_response_bytes: Option<usize>,
    proxy: Option<String>,
    no_system_proxy: bool,
}

impl ArticleBuilder {
    pub fn new<T: IntoUrl>(url: T) -> Result<Self, ArticleError> {
        let url = url.into_url()?;

        Ok(ArticleBuilder {
            url: Some(url),
            timeout: None,
            language: None,
            browser_user_agent: None,
            max_response_bytes: None,
            proxy: None,
            no_system_proxy: false,
        })
    }

    pub fn browser_user_agent<T: ToString>(mut self, browser_user_agent: T) -> Self {
        self.browser_user_agent = Some(browser_user_agent.to_string());
        self
    }

    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = Some(dur);
        self
    }

    pub fn language(mut self, language: Language) -> Self {
        self.language = Some(language);
        self
    }

    /// Bound the decoded response body retained for an article.
    pub fn max_response_bytes(mut self, limit: usize) -> Self {
        self.max_response_bytes = Some(limit);
        self
    }

    /// Disable automatic proxies from the environment unless an explicit proxy is set.
    pub fn no_system_proxy(mut self) -> Self {
        self.no_system_proxy = true;
        self
    }

    pub fn proxy<T: Into<String>>(mut self, proxy: T) -> Self {
        self.proxy = Some(proxy.into());
        self
    }

    /// Downloads the article and extract it's content using the
    /// [`crate::DefaultExtractor`].
    pub async fn get(self) -> Result<Article, ArticleError> {
        Ok(self.get_with_source().await?.article)
    }

    /// Download the article and retain its decoded response body.
    pub async fn get_with_source(self) -> Result<ArticleWithSource, ArticleError> {
        self.get_with_extractor_and_source(&DefaultExtractor::default())
            .await
    }

    /// Downloads the article and extracts it's content using the provided
    /// [`crate::Extractor`].
    pub async fn get_with_extractor<TExtract: Extractor>(
        self,
        extractor: &TExtract,
    ) -> Result<Article, ArticleError> {
        Ok(self.get_with_extractor_and_source(extractor).await?.article)
    }

    pub async fn get_with_extractor_and_source<TExtract: Extractor>(
        self,
        extractor: &TExtract,
    ) -> Result<ArticleWithSource, ArticleError> {
        let url = match self.url {
            Some(u) => u,
            None => return Err(ArticleError::UrlNotInitialized),
        };

        #[cfg(target_arch = "wasm32")]
        let builder = Client::builder();

        #[cfg(not(target_arch = "wasm32"))]
        let builder = {
            let timeout = self
                .timeout
                .unwrap_or_else(|| Duration::from_secs(Config::DEFAULT_REQUEST_TIMEOUT_SEC));

            let mut headers = HeaderMap::with_capacity(1);

            let user_agent = self
                .browser_user_agent
                .map(|x| x.parse())
                .unwrap_or_else(|| Config::user_agent().parse());
            let user_agent = match user_agent {
                Ok(ua) => ua,
                Err(_) => return Err(ArticleError::UserAgentParseError),
            };
            headers.insert(USER_AGENT, user_agent);

            let mut builder = Client::builder().default_headers(headers).timeout(timeout);
            if self.no_system_proxy {
                builder = builder.no_proxy();
            }
            if let Some(proxy) = self.proxy {
                builder = builder.proxy(reqwest::Proxy::all(proxy)?);
            }
            builder
        };

        let client = builder.build()?;
        let downloaded = crate::download::fetch_html(&client, url, self.max_response_bytes).await?;
        let url = downloaded.url;
        let raw_html = downloaded.html;
        let doc = Document::from(raw_html.as_str());

        let content = extractor
            .article_content(
                &doc,
                extractor.base_url(&doc).as_ref(),
                self.language.clone(),
            )
            .into_owned();

        // Use the detected language from content, or fall back to builder's language, or default
        let final_language = content
            .language
            .clone()
            .or(self.language)
            .unwrap_or_default();

        Ok(ArticleWithSource {
            article: Article {
                url,
                doc,
                content,
                language: final_language,
            },
            raw_html: Some(raw_html),
        })
    }
}

/// Bundles all the content found for an article.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde0", derive(Serialize, Deserialize))]
pub struct ArticleContent<'a> {
    pub authors: Vec<Cow<'a, str>>,
    pub title: Option<Cow<'a, str>>,
    pub publishing_date: Option<ArticleDate>,
    pub keywords: Vec<Cow<'a, str>>,
    pub description: Option<Cow<'a, str>>,
    pub text: Option<Cow<'a, str>>,
    pub language: Option<Language>,
    pub thumbnail: Option<Url>,
    pub top_image: Option<Url>,
    pub references: Vec<Url>,
    pub images: Vec<Url>,
    pub videos: Vec<Url>,
}

impl<'a> ArticleContent<'a> {
    /// Convenience method to create a  [`ArticleContentBuilder`]
    pub fn builder() -> ArticleContentBuilder<'a> {
        ArticleContentBuilder::default()
    }

    /// Transfers ownership of the content directly to this `ArticleContent`.
    pub fn into_owned(self) -> ArticleContent<'static> {
        ArticleContent {
            authors: self
                .authors
                .into_iter()
                .map(Cow::into_owned)
                .map(Cow::Owned)
                .collect(),
            title: self.title.map(Cow::into_owned).map(Cow::Owned),
            publishing_date: self.publishing_date,
            keywords: self
                .keywords
                .into_iter()
                .map(Cow::into_owned)
                .map(Cow::Owned)
                .collect(),
            description: self.description.map(Cow::into_owned).map(Cow::Owned),
            text: self.text.map(Cow::into_owned).map(Cow::Owned),
            language: self.language,
            thumbnail: self.thumbnail,
            top_image: self.top_image,
            references: self.references,
            images: self.images,
            videos: self.videos,
        }
    }
}

#[derive(Debug, Default)]
pub struct ArticleContentBuilder<'a> {
    pub authors: Option<Vec<Cow<'a, str>>>,
    pub title: Option<Cow<'a, str>>,
    pub publishing_date: Option<ArticleDate>,
    pub keywords: Option<Vec<Cow<'a, str>>>,
    pub description: Option<Cow<'a, str>>,
    pub text: Option<Cow<'a, str>>,
    pub language: Option<Language>,
    pub thumbnail: Option<Url>,
    pub top_image: Option<Url>,
    pub references: Option<Vec<Url>>,
    pub images: Option<Vec<Url>>,
    pub videos: Option<Vec<Url>>,
}

impl<'a> ArticleContentBuilder<'a> {
    pub fn authors(mut self, authors: Vec<Cow<'a, str>>) -> Self {
        self.authors = Some(authors);
        self
    }

    pub fn title(mut self, title: Cow<'a, str>) -> Self {
        self.title = Some(title);
        self
    }

    pub fn publishing_date(mut self, date: ArticleDate) -> Self {
        self.publishing_date = Some(date);
        self
    }

    pub fn keywords(mut self, keywords: Vec<Cow<'a, str>>) -> Self {
        self.keywords = Some(keywords);
        self
    }

    pub fn description(mut self, description: Cow<'a, str>) -> Self {
        self.description = Some(description);
        self
    }

    pub fn text(mut self, text: Cow<'a, str>) -> Self {
        self.text = Some(text);
        self
    }

    pub fn language(mut self, language: Language) -> Self {
        self.language = Some(language);
        self
    }

    pub fn thumbnail(mut self, thumbnail: Url) -> Self {
        self.thumbnail = Some(thumbnail);
        self
    }

    pub fn top_image(mut self, top_image: Url) -> Self {
        self.top_image = Some(top_image);
        self
    }

    pub fn references(mut self, references: Vec<Url>) -> Self {
        self.references = Some(references);
        self
    }

    pub fn images(mut self, images: Vec<Url>) -> Self {
        self.images = Some(images);
        self
    }

    pub fn videos(mut self, videos: Vec<Url>) -> Self {
        self.videos = Some(videos);
        self
    }

    pub fn build(self) -> ArticleContent<'a> {
        ArticleContent {
            authors: self.authors.unwrap_or_default(),
            title: self.title,
            publishing_date: self.publishing_date,
            keywords: self.keywords.unwrap_or_default(),
            description: self.description,
            text: self.text,
            language: self.language,
            thumbnail: self.thumbnail,
            top_image: self.top_image,
            references: self.references.unwrap_or_default(),
            images: self.images.unwrap_or_default(),
            videos: self.videos.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    const HTML: &str = "<!doctype html><html lang='en'><head><title>Fixture article title</title></head><body><nav>Navigation</nav><h1>Actual <em>article</em> title</h1><div itemprop='articleBody'><p>The article contains an <strong>important</strong> statement and a <a href='/source'>source link</a> with enough information to identify the article content.</p><script>sourceScript()</script></div></body></html>";

    fn serve_once(
        body: Vec<u8>,
        status: u16,
        delay: Duration,
    ) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|value| value == b"\r\n\r\n") {
                let read = socket.read(&mut buffer).unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
            }
            thread::sleep(delay);
            let headers = format!(
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(headers.as_bytes());
            let _ = socket.write_all(&body);
            String::from_utf8(request).unwrap()
        });
        (url, handle)
    }

    #[test]
    fn supplied_source_is_preserved_separately_from_article_markup() {
        let parsed = ArticleWithSource::new("https://example.com/news/test", HTML).unwrap();
        assert_eq!(parsed.raw_html.as_deref(), Some(HTML));
        let markup = parsed.article.article_html().unwrap();
        assert!(markup.contains("<strong>important</strong>"));
        assert!(markup.contains("href=\"/source\""));
        assert!(!markup.contains("Navigation"));
        assert!(markup.contains("sourceScript()"));
    }

    #[tokio::test]
    async fn builder_returns_article_and_source_from_one_request() {
        let (url, server) = serve_once(HTML.as_bytes().to_vec(), 200, Duration::ZERO);
        let parsed = Article::builder(&url)
            .unwrap()
            .no_system_proxy()
            .get_with_source()
            .await
            .unwrap();
        assert_eq!(parsed.raw_html.as_deref(), Some(HTML));
        assert_eq!(
            parsed.article.content.title.as_deref(),
            Some("Actual article title")
        );
        assert_eq!(parsed.article.language, Language::English);
        assert!(server.join().unwrap().starts_with("GET / HTTP/1.1"));
    }

    struct FixtureExtractor;

    impl Extractor for FixtureExtractor {
        fn title<'a>(&self, _doc: &'a Document) -> Option<Cow<'a, str>> {
            Some(Cow::Borrowed("Custom extractor title"))
        }
    }

    #[tokio::test]
    async fn custom_extractor_works_with_source_and_existing_get() {
        let (url, server) = serve_once(HTML.as_bytes().to_vec(), 200, Duration::ZERO);
        let parsed = Article::builder(&url)
            .unwrap()
            .no_system_proxy()
            .get_with_extractor_and_source(&FixtureExtractor)
            .await
            .unwrap();
        server.join().unwrap();
        assert_eq!(parsed.raw_html.as_deref(), Some(HTML));
        assert_eq!(
            parsed.article.content.title.as_deref(),
            Some("Custom extractor title")
        );

        let (url, server) = serve_once(HTML.as_bytes().to_vec(), 200, Duration::ZERO);
        let parsed = Article::builder(&url)
            .unwrap()
            .no_system_proxy()
            .get_with_extractor(&FixtureExtractor)
            .await
            .unwrap();
        server.join().unwrap();
        assert_eq!(
            parsed.content.title.as_deref(),
            Some("Custom extractor title")
        );
    }

    #[tokio::test]
    async fn builder_preserves_typed_timeout() {
        let (url, server) = serve_once(HTML.as_bytes().to_vec(), 200, Duration::from_millis(150));
        let error = Article::builder(&url)
            .unwrap()
            .no_system_proxy()
            .timeout(Duration::from_millis(25))
            .get_with_source()
            .await
            .unwrap_err();
        server.join().unwrap();
        assert!(matches!(error, ArticleError::Reqwest(error) if error.is_timeout()));
    }

    #[tokio::test]
    async fn builder_preserves_typed_response_limit() {
        let (url, server) = serve_once(HTML.as_bytes().to_vec(), 200, Duration::ZERO);
        let error = Article::builder(&url)
            .unwrap()
            .no_system_proxy()
            .max_response_bytes(64)
            .get_with_source()
            .await
            .unwrap_err();
        server.join().unwrap();
        assert!(matches!(
            error,
            ArticleError::LimitExceeded { limit: 64, .. }
        ));
    }

    #[tokio::test]
    async fn builder_uses_explicit_proxy_and_user_agent() {
        let (proxy, server) = serve_once(HTML.as_bytes().to_vec(), 200, Duration::ZERO);
        let parsed = Article::builder("http://does-not-resolve.invalid/news/story")
            .unwrap()
            .no_system_proxy()
            .proxy(proxy)
            .browser_user_agent("FixtureClient/1")
            .get_with_source()
            .await
            .unwrap();
        let request = server.join().unwrap();
        assert!(request.starts_with("GET http://does-not-resolve.invalid/news/story HTTP/1.1"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("user-agent: fixtureclient/1")
        );
        assert_eq!(parsed.raw_html.as_deref(), Some(HTML));
    }

    #[tokio::test]
    async fn http_error_retains_status_without_response_body() {
        let (url, server) = serve_once(b"Internal server details".to_vec(), 503, Duration::ZERO);
        let error = Article::builder(&url)
            .unwrap()
            .no_system_proxy()
            .get_with_source()
            .await
            .unwrap_err();
        server.join().unwrap();
        assert!(matches!(
            error,
            ArticleError::HttpStatus(reqwest::StatusCode::SERVICE_UNAVAILABLE)
        ));
    }
}
