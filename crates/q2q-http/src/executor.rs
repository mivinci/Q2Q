use crate::Error;
use bytes::{Bytes, BytesMut};
use reqwest::{
  StatusCode,
  header::{
    ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_RANGE, ETAG, IF_RANGE, LOCATION, RANGE, RETRY_AFTER,
  },
};
use std::{
  collections::HashMap,
  ops::Range,
  sync::{Arc, Mutex, Weak},
  time::{Duration, SystemTime},
};
use tokio::{
  sync::{OwnedSemaphorePermit, Semaphore},
  time::{Instant, timeout_at},
};
use url::Url;

type UrlPolicy = Arc<dyn Fn(&Url) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct Executor {
  inner: Arc<Inner>,
  url_policy: Option<UrlPolicy>,
}
struct Inner {
  http: reqwest::Client,
  global: Arc<Semaphore>,
  hosts: Mutex<HashMap<String, Weak<Semaphore>>>,
  per_host: usize,
  timeout: Duration,
  redirects: usize,
  collect_limit: usize,
  limit: usize,
}
/// Budgets and connection pool are shared by all clones. Build does not connect.
pub struct ExecutorBuilder {
  requests: usize,
  per_host: usize,
  timeout: Duration,
  connect_timeout: Duration,
  redirects: usize,
  collect_limit: usize,
  invalid_certs: bool,
  url_policy: Option<UrlPolicy>,
}
impl Default for ExecutorBuilder {
  fn default() -> Self {
    Self {
      requests: 16,
      per_host: 8,
      timeout: Duration::from_secs(20),
      connect_timeout: Duration::from_secs(5),
      redirects: 3,
      collect_limit: 8 * 1024 * 1024,
      invalid_certs: false,
      url_policy: None,
    }
  }
}
impl ExecutorBuilder {
  pub fn max_requests(mut self, n: usize) -> Self {
    self.requests = n;
    self
  }
  /// Budget per host/port, shared across redirects and cloned handles.
  pub fn max_requests_per_host(mut self, n: usize) -> Self {
    self.per_host = n;
    self
  }
  pub fn timeout(mut self, d: Duration) -> Self {
    self.timeout = d;
    self
  }
  pub fn connect_timeout(mut self, d: Duration) -> Self {
    self.connect_timeout = d;
    self
  }
  pub fn max_redirects(mut self, n: usize) -> Self {
    self.redirects = n;
    self
  }
  pub fn collect_limit(mut self, n: usize) -> Self {
    self.collect_limit = n;
    self
  }
  /// Disable TLS verification for debugging only. Off by default.
  pub fn danger_accept_invalid_certs(mut self, yes: bool) -> Self {
    self.invalid_certs = yes;
    self
  }
  /// Check each initial/redirect URL before admission or network I/O.
  /// The synchronous predicate must be fast and must not block. Rejection errors
  /// contain no URL. This is a URL policy, not DNS/IP pinning or an SSRF sandbox.
  pub fn url_policy(mut self, policy: impl Fn(&Url) -> bool + Send + Sync + 'static) -> Self {
    self.url_policy = Some(Arc::new(policy));
    self
  }
  pub fn build(self) -> Result<Executor, Error> {
    if self.requests == 0
      || self.per_host == 0
      || self.requests > Semaphore::MAX_PERMITS
      || self.per_host > Semaphore::MAX_PERMITS
      || self.timeout.is_zero()
      || self.connect_timeout.is_zero()
      || self.collect_limit == 0
    {
      return Err(Error::Invalid(
        "budgets and timeouts must be positive and within platform limits",
      ));
    }
    let http = reqwest::Client::builder()
      .danger_accept_invalid_certs(self.invalid_certs)
      .connect_timeout(self.connect_timeout)
      .redirect(reqwest::redirect::Policy::none())
      // Retry policy belongs to the file downloader or Q2Q source strategy;
      // disable reqwest's protocol-NACK retries as well as application retries.
      .retry(reqwest::retry::never())
      .referer(false)
      .build()
      .map_err(Error::http)?;
    Ok(Executor {
      url_policy: self.url_policy,
      inner: Arc::new(Inner {
        http,
        global: Arc::new(Semaphore::new(self.requests)),
        hosts: Mutex::new(HashMap::new()),
        per_host: self.per_host,
        timeout: self.timeout,
        redirects: self.redirects,
        collect_limit: self.collect_limit,
        limit: self.requests,
      }),
    })
  }
}
impl Executor {
  /// Derive a restricted handle sharing the pool and all admission budgets.
  /// Additional policies are intersected; a derived handle cannot loosen its parent.
  pub fn with_url_policy(&self, policy: impl Fn(&Url) -> bool + Send + Sync + 'static) -> Self {
    let previous = self.url_policy.clone();
    Self {
      inner: self.inner.clone(),
      url_policy: Some(Arc::new(move |url| {
        previous.as_ref().is_none_or(|check| check(url)) && policy(url)
      })),
    }
  }
  pub fn builder() -> ExecutorBuilder {
    ExecutorBuilder::default()
  }
  pub fn get(&self, url: impl AsRef<str>) -> Request {
    Request {
      executor: self.clone(),
      url: url.as_ref().into(),
      range: None,
      expected: None,
      max_body: self.inner.collect_limit as u64,
      timeout: self.inner.timeout,
      validator: None,
    }
  }
  /// End-exclusive range; a 200 is accepted only when it covers the whole file.
  pub fn get_range(&self, url: impl AsRef<str>, range: Range<u64>, total: u64) -> Request {
    let mut request = self.get(url);
    request.expected = Some(range.end.saturating_sub(range.start));
    request.max_body = request.expected.unwrap();
    request.range = Some((range, total));
    request
  }
  pub fn active_requests(&self) -> usize {
    self.inner.limit - self.inner.global.available_permits()
  }
  async fn permits(
    &self,
    url: &Url,
  ) -> Result<(OwnedSemaphorePermit, OwnedSemaphorePermit), Error> {
    let key = format!(
      "{}:{}",
      url.host_str().unwrap(),
      url.port_or_known_default().unwrap()
    );
    let host = {
      let mut hosts = self.inner.hosts.lock().unwrap_or_else(|e| e.into_inner());
      hosts.retain(|_, value| value.strong_count() > 0);
      if let Some(host) = hosts.get(&key).and_then(Weak::upgrade) {
        host
      } else {
        let host = Arc::new(Semaphore::new(self.inner.per_host));
        hosts.insert(key, Arc::downgrade(&host));
        host
      }
    };
    // Everyone takes host then global; redirects drop both before taking another
    // host. Never hold global capacity while waiting for a saturated host.
    let host = host.acquire_owned().await.map_err(|_| Error::Cancelled)?;
    let global = self
      .inner
      .global
      .clone()
      .acquire_owned()
      .await
      .map_err(|_| Error::Cancelled)?;
    Ok((global, host))
  }
}
pub(crate) fn parse_url(value: &str) -> Result<Url, Error> {
  let url = Url::parse(value).map_err(|_| Error::Invalid("invalid HTTP URL"))?;
  if !matches!(url.scheme(), "http" | "https")
    || url.host_str().is_none()
    || !url.username().is_empty()
    || url.password().is_some()
  {
    return Err(Error::Invalid(
      "source must be HTTP(S) without embedded credentials",
    ));
  }
  Ok(url)
}
pub(crate) fn strong_etag(value: &str) -> bool {
  let bytes = value.as_bytes();
  bytes.len() >= 2
    && bytes[0] == b'"'
    && bytes[bytes.len() - 1] == b'"'
    && bytes[1..bytes.len() - 1]
      .iter()
      .all(|b| *b == 0x21 || (0x23..=0x7e).contains(b) || *b >= 0x80)
}
/// A request has no background work until send is polled. No implicit retries.
pub struct Request {
  executor: Executor,
  url: String,
  range: Option<(Range<u64>, u64)>,
  expected: Option<u64>,
  max_body: u64,
  timeout: Duration,
  validator: Option<String>,
}
impl Request {
  pub fn expected_size(mut self, n: u64) -> Self {
    self.expected = Some(n);
    self.max_body = n;
    self
  }
  pub fn max_body_size(mut self, n: u64) -> Self {
    self.max_body = n;
    self
  }
  /// Total deadline includes request admission, redirects and body consumption.
  pub fn timeout(mut self, d: Duration) -> Self {
    self.timeout = d;
    self
  }
  /// Used by the file downloader to bind resumed ranges to a strong ETag.
  pub fn if_range(mut self, etag: impl Into<String>) -> Self {
    self.validator = Some(etag.into());
    self
  }
  pub async fn send(self) -> Result<Response, Error> {
    let deadline = Instant::now()
      .checked_add(self.timeout)
      .ok_or(Error::Invalid("timeout too large"))?;
    timeout_at(deadline, self.send_inner(deadline))
      .await
      .map_err(|_| Error::Timeout)?
  }
  async fn send_inner(self, deadline: Instant) -> Result<Response, Error> {
    let mut url = parse_url(&self.url)?;
    if self.validator.as_deref().is_some_and(|v| !strong_etag(v)) {
      return Err(Error::Invalid("If-Range requires a strong ETag"));
    }
    if let Some((r, total)) = &self.range
      && (r.start >= r.end
        || r.end > *total
        || self.expected != Some(r.end - r.start)
        || self.max_body < r.end - r.start)
    {
      return Err(Error::Invalid("invalid byte range"));
    }
    for redirects in 0..=self.executor.inner.redirects {
      if self
        .executor
        .url_policy
        .as_ref()
        .is_some_and(|check| !check(&url))
      {
        return Err(Error::Invalid("request URL rejected by policy"));
      }
      let permits = self.executor.permits(&url).await?;
      let mut request = self
        .executor
        .inner
        .http
        .get(url.clone())
        .header(ACCEPT_ENCODING, "identity");
      if let Some((r, _)) = &self.range {
        request = request.header(RANGE, format!("bytes={}-{}", r.start, r.end - 1));
      }
      if let Some(value) = &self.validator {
        request = request.header(IF_RANGE, value);
      }
      let response = request.send().await.map_err(Error::http)?;
      if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
        if redirects == self.executor.inner.redirects {
          return Err(Error::Protocol("redirect limit exceeded"));
        }
        let location = response
          .headers()
          .get(LOCATION)
          .and_then(|v| v.to_str().ok())
          .ok_or(Error::Protocol("missing redirect location"))?;
        let next = url
          .join(location)
          .map_err(|_| Error::Protocol("invalid redirect URL"))?;
        url = parse_url(next.as_str())?;
        drop(response);
        drop(permits);
        continue;
      }
      let status = response.status();
      if !status.is_success() {
        let retry_after = response
          .headers()
          .get(RETRY_AFTER)
          .and_then(|v| v.to_str().ok())
          .and_then(|s| {
            s.parse::<u64>().ok().map(Duration::from_secs).or_else(|| {
              httpdate::parse_http_date(s)
                .ok()
                .map(|t| t.duration_since(SystemTime::now()).unwrap_or_default())
            })
          })
          .map(|d| d.min(Duration::from_secs(30)));
        return Err(Error::Status {
          code: status.as_u16(),
          retry_after,
        });
      }
      if let Some((r, total)) = &self.range {
        if status == StatusCode::OK && !(r.start == 0 && r.end == *total) {
          return Err(Error::RangeUnsupported);
        }
        if status == StatusCode::PARTIAL_CONTENT {
          let expected = format!("bytes {}-{}/{total}", r.start, r.end - 1);
          if response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            != Some(expected.as_str())
          {
            return Err(Error::Protocol(
              "Content-Range does not match range and total length",
            ));
          }
        } else if status != StatusCode::OK {
          return Err(Error::Protocol("range requires 206"));
        }
      } else if status != StatusCode::OK {
        return Err(Error::Protocol("full GET requires 200"));
      }
      if response
        .headers()
        .get(CONTENT_ENCODING)
        .is_some_and(|v| !v.as_bytes().eq_ignore_ascii_case(b"identity"))
      {
        return Err(Error::Protocol("compressed representation"));
      }
      if let Some(n) = response.content_length()
        && (n > self.max_body || self.expected.is_some_and(|expected| n != expected))
      {
        return Err(Error::Protocol("response length does not match request"));
      }
      let etag = response
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .filter(|s| strong_etag(s))
        .map(str::to_owned);
      if let Some(expected) = &self.validator
        && etag.as_ref() != Some(expected)
      {
        return Err(Error::SourceChanged);
      }
      return Ok(Response {
        body: Some(response),
        permits: Some(permits),
        deadline,
        expected: self.expected,
        max_body: self.max_body,
        read: 0,
        collect_limit: self.executor.inner.collect_limit,
        etag,
        failed: false,
      });
    }
    unreachable!()
  }
}
/// Owns request permits until checked EOF, failure, or Drop. Bytes remain untrusted.
pub struct Response {
  body: Option<reqwest::Response>,
  permits: Option<(OwnedSemaphorePermit, OwnedSemaphorePermit)>,
  deadline: Instant,
  expected: Option<u64>,
  max_body: u64,
  read: u64,
  collect_limit: usize,
  etag: Option<String>,
  failed: bool,
}
impl Response {
  pub fn etag(&self) -> Option<&str> {
    self.etag.as_deref()
  }
  pub async fn chunk(&mut self) -> Result<Option<Bytes>, Error> {
    if self.failed {
      return Err(Error::Protocol("response previously failed"));
    }
    let Some(body) = self.body.as_mut() else {
      return Ok(None);
    };
    let result = timeout_at(self.deadline, body.chunk())
      .await
      .map_err(|_| Error::Timeout)
      .and_then(|r| r.map_err(Error::http));
    let result = match result {
      Ok(Some(bytes)) if bytes.len() as u64 <= self.max_body.saturating_sub(self.read) => {
        self.read += bytes.len() as u64;
        return Ok(Some(bytes));
      }
      Ok(Some(_)) => Err(Error::Protocol("response exceeds allowed length")),
      Ok(None) if self.expected.is_some_and(|n| n != self.read) => {
        Err(Error::Protocol("response ended before expected length"))
      }
      Ok(None) => Ok(None),
      Err(e) => Err(e),
    };
    self.failed = result.is_err();
    self.body.take();
    self.permits.take();
    result
  }
  pub async fn collect_exact(mut self, length: usize) -> Result<Bytes, Error> {
    if self.read != 0
      || length > self.collect_limit
      || length as u64 > self.max_body
      || self.expected.is_some_and(|n| n != length as u64)
    {
      return Err(Error::Invalid(
        "collection length exceeds limit or contradicts request",
      ));
    }
    self.expected = Some(length as u64);
    self.max_body = length as u64;
    let mut bytes = BytesMut::new();
    while let Some(chunk) = self.chunk().await? {
      bytes.extend_from_slice(&chunk);
    }
    Ok(bytes.freeze())
  }
  /// Collect unknown-length metadata with an explicit allocation bound.
  pub async fn collect_bounded(mut self, limit: usize) -> Result<Bytes, Error> {
    if self.read != 0 || limit > self.collect_limit {
      return Err(Error::Invalid("collection exceeds configured limit"));
    }
    self.max_body = self.max_body.min(limit as u64);
    let mut bytes = BytesMut::new();
    while let Some(chunk) = self.chunk().await? {
      bytes.extend_from_slice(&chunk);
    }
    Ok(bytes.freeze())
  }
}
