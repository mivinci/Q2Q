//! A tower layer so any HTTP service can drop this in.
//!
//! Identifying the peer is a separate step on purpose: behind a load balancer
//! the socket address is the proxy's, which would collapse every peer into one
//! bucket. `with_identifier` is the single place a deployment changes to read
//! `X-Forwarded-For` instead.

use crate::limiter::{Key, Limiter, Millis};
use crate::network::NetworkKey;
use axum::body::Body;
use axum::extract::ConnectInfo;
use http::{HeaderValue, Request, Response, StatusCode};
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{SystemTime, UNIX_EPOCH};
use tower::{Layer, Service};

pub fn now_ms() -> Millis {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_millis() as Millis)
    .unwrap_or(0)
}

/// Fallback extension, for callers that do not go through `ConnectInfo`.
#[derive(Debug, Clone, Copy)]
pub struct PeerAddr(pub IpAddr);

fn peer_addr(req: &Request<()>) -> Option<IpAddr> {
  req
    .extensions()
    .get::<ConnectInfo<SocketAddr>>()
    .map(|c| c.0.ip())
    .or_else(|| req.extensions().get::<PeerAddr>().map(|p| p.0))
}

type IdentifyFn = Arc<dyn Fn(&Request<()>) -> Option<Key> + Send + Sync>;

#[derive(Clone)]
pub struct RateLimitLayer {
  limiter: Arc<Limiter>,
  rule: Arc<str>,
  cost: u32,
  identify: IdentifyFn,
}

impl RateLimitLayer {
  pub fn new(limiter: Arc<Limiter>, rule: impl Into<Arc<str>>) -> Self {
    RateLimitLayer {
      limiter,
      rule: rule.into(),
      cost: 1,
      identify: Arc::new(|req| peer_addr(req).map(|ip| Key::network(NetworkKey::of(ip)))),
    }
  }

  pub fn with_cost(mut self, cost: u32) -> Self {
    self.cost = cost;
    self
  }

  /// Replace how the caller is identified. When reading `X-Forwarded-For`, take
  /// the address N hops from the right — N being the number of proxies you
  /// control. The leftmost entry comes from the client and can be forged.
  pub fn with_identifier(
    mut self,
    f: impl Fn(&Request<()>) -> Option<Key> + Send + Sync + 'static,
  ) -> Self {
    self.identify = Arc::new(f);
    self
  }
}

impl<S> Layer<S> for RateLimitLayer {
  type Service = RateLimitService<S>;

  fn layer(&self, inner: S) -> Self::Service {
    RateLimitService {
      inner,
      limiter: self.limiter.clone(),
      rule: self.rule.clone(),
      cost: self.cost,
      identify: self.identify.clone(),
    }
  }
}

#[derive(Clone)]
pub struct RateLimitService<S> {
  inner: S,
  limiter: Arc<Limiter>,
  rule: Arc<str>,
  cost: u32,
  identify: IdentifyFn,
}

impl<S, B> Service<Request<B>> for RateLimitService<S>
where
  S: Service<Request<B>, Response = Response<Body>> + Clone + Send + 'static,
  S::Future: Send + 'static,
  B: Send + 'static,
{
  type Response = Response<Body>;
  type Error = S::Error;
  type Future = Pin<Box<dyn Future<Output = Result<Response<Body>, S::Error>> + Send>>;

  fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
    self.inner.poll_ready(cx)
  }

  fn call(&mut self, req: Request<B>) -> Self::Future {
    let (parts, body) = req.into_parts();
    let probe = Request::from_parts(parts.clone(), ());

    let rejected = match (self.identify)(&probe) {
      Some(key) => {
        let verdict = self.limiter.check(&self.rule, &key, self.cost, now_ms());
        if verdict.allowed {
          None
        } else {
          Some(rejection(verdict.retry_after_ms))
        }
      }
      // no address to key on: let it through rather than break the deployment
      None => None,
    };

    match rejected {
      Some(resp) => Box::pin(async move { Ok(resp) }),
      None => {
        let mut inner = self.inner.clone();
        Box::pin(inner.call(Request::from_parts(parts, body)))
      }
    }
  }
}

fn rejection(retry_after_ms: Option<Millis>) -> Response<Body> {
  let mut resp = Response::new(Body::from("{\"code\":\"rate_limited\"}"));
  *resp.status_mut() = StatusCode::TOO_MANY_REQUESTS;
  resp.headers_mut().insert(
    http::header::CONTENT_TYPE,
    HeaderValue::from_static("application/json"),
  );
  if let Some(ms) = retry_after_ms {
    let secs = ms.div_ceil(1000).max(1);
    if let Ok(value) = HeaderValue::from_str(&secs.to_string()) {
      resp.headers_mut().insert(http::header::RETRY_AFTER, value);
    }
  }
  resp
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::limiter::Rule;
  use std::collections::HashMap;

  #[test]
  fn now_ms_looks_like_a_real_timestamp() {
    assert!(now_ms() > 1_600_000_000_000);
  }

  fn probe(limit: u32) -> (Arc<Limiter>, RateLimitLayer) {
    let mut rules = HashMap::new();
    rules.insert("query".to_string(), Rule::per_minute(limit, 60));
    let l = Arc::new(Limiter::new(rules));
    let layer = RateLimitLayer::new(l.clone(), "query");
    (l, layer)
  }

  fn request_with(ip: IpAddr) -> Request<()> {
    let mut req = Request::new(());
    req.extensions_mut().insert(PeerAddr(ip));
    req
  }

  #[test]
  fn identifies_the_peer_from_the_extension() {
    let ip: IpAddr = "198.51.100.7".parse().unwrap();
    let req = request_with(ip);
    assert_eq!(peer_addr(&req), Some(ip));
  }

  #[test]
  fn layer_denies_once_the_bucket_is_spent() {
    let (_, layer) = probe(1);
    let ip: IpAddr = "198.51.100.7".parse().unwrap();
    let identify = layer.identify.clone();

    let key = identify(&request_with(ip)).unwrap();
    let limiter = layer.limiter.clone();
    assert!(limiter.check("query", &key, 1, now_ms()).allowed);
    assert!(!limiter.check("query", &key, 1, now_ms()).allowed);
  }

  #[test]
  fn custom_identifier_can_replace_the_default() {
    let (_, layer) = probe(10);
    let layer = layer.with_identifier(|_req| {
      let ip: IpAddr = "203.0.113.9".parse().unwrap();
      Some(Key::network(NetworkKey::of(ip)))
    });
    let key = (layer.identify)(&Request::new(())).unwrap();
    assert_eq!(
      key,
      Key::network(NetworkKey::of("203.0.113.9".parse().unwrap()))
    );
  }

  #[test]
  fn rejection_carries_retry_after() {
    let resp = rejection(Some(2_500));
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    // rounded up so a client never retries too early
    assert_eq!(resp.headers().get(http::header::RETRY_AFTER).unwrap(), "3");
  }
}
