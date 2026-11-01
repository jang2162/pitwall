//! 받기 — HTTPS 만(리다이렉트도), 크기 상한(결정 M4-D17).
//!
//! registry 산출물(M4-03)이 쓰고, 설치기(M4-04)가 같은 규칙 위에 넓힌다(파일로 흘려 쓰기 · 큰 상한). 받은 바이트는 미신뢰다 —
//! 해시 확인은 부르는 쪽이 한다.

use std::time::Duration;

use sha2::{Digest, Sha256};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_REDIRECTS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// 리다이렉트가 상한([`MAX_REDIRECT_HOPS`])을 넘었다.
    TooManyRedirects,
    /// `https://` 가 아니다(리다이렉트 대상 포함).
    Insecure(String),
    /// 상한을 넘었다.
    TooLarge { limit: usize },
    /// 2xx 가 아닌 답.
    Status(u16),
    /// 연결 · 시한 · TLS 등.
    Network(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::TooManyRedirects => write!(f, "more than {MAX_REDIRECTS} redirects"),
            FetchError::Insecure(url) => write!(f, "refusing a non-https URL: {url}"),
            FetchError::TooLarge { limit } => write!(f, "response is larger than {limit} bytes"),
            FetchError::Status(code) => write!(f, "server answered {code}"),
            FetchError::Network(e) => write!(f, "network error: {e}"),
        }
    }
}

impl std::error::Error for FetchError {}

/// 리다이렉트를 따라가는 횟수의 상한.
pub const MAX_REDIRECT_HOPS: usize = MAX_REDIRECTS;

#[derive(Clone)]
pub struct Fetcher {
    client: reqwest::Client,
    loopback_http: bool,
}

/// `https://` 이거나, 시험에서만 `http://127.0.0.1`.
fn allowed(url: &reqwest::Url, loopback_http: bool) -> bool {
    match url.scheme() {
        "https" => url.host_str().is_some_and(|h| !h.is_empty()),
        "http" => {
            loopback_http
                && matches!(
                    url.host(),
                    Some(url::Host::Ipv4(ip)) if ip.is_loopback()
                )
        }
        _ => false,
    }
}

impl Fetcher {
    pub fn new() -> Result<Fetcher, FetchError> {
        Fetcher::build(false)
    }

    /// 시험의 가짜 HTTP 서버(`http://127.0.0.1:<포트>`)를 받는다. 그 밖은 [`Fetcher::new`] 와 같다.
    #[cfg(test)]
    pub(crate) fn for_loopback_tests() -> Fetcher {
        Fetcher::build(true).unwrap()
    }

    /// 시한을 줄인 시험용.
    #[cfg(test)]
    pub(crate) fn for_loopback_tests_with_timeout(timeout: Duration) -> Fetcher {
        Fetcher::build_with(true, timeout).unwrap()
    }

    fn build(loopback_http: bool) -> Result<Fetcher, FetchError> {
        Fetcher::build_with(loopback_http, REQUEST_TIMEOUT)
    }

    fn build_with(loopback_http: bool, timeout: Duration) -> Result<Fetcher, FetchError> {
        // 데스크톱 업데이터와 같은 암호 제공자(ring). 이미 걸려 있으면 그대로 둔다.
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let policy = reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() > MAX_REDIRECTS {
                attempt.error(FetchError::TooManyRedirects)
            } else if !allowed(attempt.url(), loopback_http) {
                let url = attempt.url().to_string();
                attempt.error(FetchError::Insecure(url))
            } else {
                attempt.follow()
            }
        });
        let mut builder = reqwest::Client::builder();
        if loopback_http {
            // 시험의 가짜 서버(127.0.0.1)에 사람 · CI 의 프록시 설정이 끼지 않게.
            builder = builder.no_proxy();
        }
        let client = builder
            .user_agent(concat!("pitwall/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT.min(timeout))
            .timeout(timeout)
            .redirect(policy)
            .build()
            .map_err(|e| FetchError::Network(e.to_string()))?;
        Ok(Fetcher {
            client,
            loopback_http,
        })
    }

    /// `url` 을 받아 바이트로. `limit` 을 넘으면 받다가 멈춘다.
    pub async fn get(
        &self,
        url: &str,
        limit: usize,
        accept: Option<&str>,
    ) -> Result<Vec<u8>, FetchError> {
        let parsed = reqwest::Url::parse(url).map_err(|_| FetchError::Insecure(url.to_owned()))?;
        if !allowed(&parsed, self.loopback_http) {
            return Err(FetchError::Insecure(url.to_owned()));
        }
        let mut request = self.client.get(parsed);
        if let Some(accept) = accept {
            request = request.header("Accept", accept);
        }
        let mut response = request.send().await.map_err(|e| classify(&e))?;
        let status = response.status();
        if !status.is_success() {
            return Err(FetchError::Status(status.as_u16()));
        }
        if response.content_length().is_some_and(|n| n > limit as u64) {
            return Err(FetchError::TooLarge { limit });
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| classify(&e))? {
            if body.len() + chunk.len() > limit {
                return Err(FetchError::TooLarge { limit });
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

/// 리다이렉트 정책이 낸 [`FetchError::Insecure`] 를 꺼낸다.
fn classify(e: &reqwest::Error) -> FetchError {
    let mut source: Option<&dyn std::error::Error> = Some(e);
    while let Some(s) = source {
        if let Some(f) = s.downcast_ref::<FetchError>() {
            return f.clone();
        }
        source = s.source();
    }
    FetchError::Network(e.to_string())
}

/// 소문자 16진수 SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}
