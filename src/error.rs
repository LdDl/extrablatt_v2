use bytes::Bytes;
use thiserror::Error;

use crate::article::PureArticle;

/// All different error types this crate uses.
#[derive(Error, Debug)]
pub enum ExtrablattError {
    /// Received a good non success Http response
    #[error("Expected a 2xx Success but got: {}", response.status())]
    NoHttpSuccessResponse {
        /// The good reqwest response.
        response: reqwest::Response,
    },
    /// Failed to get a response.
    #[error("Request failed: {error}")]
    HttpRequestFailure {
        /// The reqwest error.
        error: reqwest::Error,
    },
    /// Failed to read a document.
    #[error("Failed to read document")]
    ReadDocumentError {
        /// The content the resulted in the error.
        body: Bytes,
    },
    /// Identified an article, but it's content doesn't fulfill the configured
    /// requirements.
    #[error("Found incomplete Article for {}", article.url)]
    IncompleteArticle {
        /// The found article and its content.
        article: Box<PureArticle>,
    },
    /// The base URL was not initialized.
    #[error("Url of the article must be initialized.")]
    UrlNotInitialized,
    /// The base URL is invalid.
    #[error("url {url:?} can not be a base url")]
    BaseUrlInvalid {
        url: reqwest::Url,
    },
    /// Failed to parse user agent header.
    #[error("Failed to parse user agent header")]
    UserAgentParseError,
    /// Error from reqwest.
    #[error("Reqwest error: {0}")]
    Reqwest(#[source] reqwest::Error),
    /// Error parsing URL.
    #[error("Failed to parse URL: {error}")]
    UrlParseError {
        error: reqwest::Error,
    },
}
