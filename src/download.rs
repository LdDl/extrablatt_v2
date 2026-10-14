use std::sync::{Arc, Mutex};

use reqwest::{Client, Url};

use crate::ExtrablattError;

/// A decoded HTTP body and its final URL after redirects.
#[derive(Debug)]
pub struct DownloadedHtml {
    pub url: Url,
    pub html: String,
}

#[derive(Debug, Clone)]
pub(crate) struct DownloadBudget {
    limit: Option<usize>,
    used: Arc<Mutex<usize>>,
}

impl DownloadBudget {
    pub(crate) fn new(limit: Option<usize>) -> Self {
        Self {
            limit,
            used: Arc::new(Mutex::new(0)),
        }
    }

    pub(crate) fn used(&self) -> usize {
        *self.used.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn charge(&self, bytes: usize) -> Result<(), ExtrablattError> {
        let mut used = self.used.lock().unwrap_or_else(|error| error.into_inner());
        let next = used.saturating_add(bytes);
        if let Some(limit) = self.limit {
            if next > limit {
                return Err(ExtrablattError::LimitExceeded {
                    resource: "total_bytes",
                    limit,
                });
            }
        }
        *used = next;
        Ok(())
    }
}

/// Download once, enforcing the limit while reading and after charset decoding.
/// On wasm, the browser buffers the body before the post-read size check.
pub async fn fetch_html(
    client: &Client,
    url: Url,
    limit: Option<usize>,
) -> Result<DownloadedHtml, ExtrablattError> {
    fetch_html_with_budget(client, url, limit, &DownloadBudget::new(None), true).await
}

pub(crate) async fn fetch_html_with_budget(
    client: &Client,
    url: Url,
    limit: Option<usize>,
    budget: &DownloadBudget,
    http_success_only: bool,
) -> Result<DownloadedHtml, ExtrablattError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| ExtrablattError::HttpRequestFailure { error })?;
    if http_success_only && !response.status().is_success() {
        return Err(ExtrablattError::NoHttpSuccessResponse { response });
    }
    let url = response.url().clone();
    if let (Some(limit), Some(length)) = (limit, response.content_length()) {
        if length > limit as u64 {
            return Err(ExtrablattError::LimitExceeded {
                resource: "response_bytes",
                limit,
            });
        }
    }
    #[cfg(target_arch = "wasm32")]
    let html = {
        let html = response
            .text()
            .await
            .map_err(|error| ExtrablattError::HttpRequestFailure { error })?;
        if let Some(limit) = limit {
            if html.len() > limit {
                return Err(ExtrablattError::LimitExceeded {
                    resource: "response_bytes",
                    limit,
                });
            }
        }
        budget.charge(html.len())?;
        html
    };
    #[cfg(not(target_arch = "wasm32"))]
    let html = {
        let mut response = response;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .cloned();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| ExtrablattError::HttpRequestFailure { error })?
        {
            if let Some(limit) = limit {
                if bytes.len().saturating_add(chunk.len()) > limit {
                    return Err(ExtrablattError::LimitExceeded {
                        resource: "response_bytes",
                        limit,
                    });
                }
            }
            budget.charge(chunk.len())?;
            bytes.extend_from_slice(&chunk);
        }
        let raw_len = bytes.len();
        let mut decoded = http::Response::builder();
        if let Some(content_type) = content_type {
            decoded = decoded.header(reqwest::header::CONTENT_TYPE, content_type);
        }
        let response =
            reqwest::Response::from(decoded.body(bytes).expect("valid buffered response"));
        let html = response
            .text()
            .await
            .map_err(|error| ExtrablattError::HttpRequestFailure { error })?;
        if let Some(limit) = limit {
            if html.len() > limit {
                return Err(ExtrablattError::LimitExceeded {
                    resource: "response_bytes",
                    limit,
                });
            }
        }
        // Charset decoding can expand a body; account for the larger retained representation.
        budget.charge(html.len().saturating_sub(raw_len))?;
        html
    };
    Ok(DownloadedHtml { url, html })
}
