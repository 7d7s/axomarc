//! # kelson-webhooks
//! Outbound webhooks (blueprint L44 companion, S3): applications subscribe
//! external URLs to platform events; the dispatcher signs and delivers
//! payloads over the kelson-http-client port (timeout + retry centralized).
//!
//! Security: every delivery is signed with HMAC-SHA256 over the exact body
//! bytes using the subscription secret; receivers verify before trusting
//! anything. The signature header carries `sha256=<hex>` (GitHub-style).

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use kelson_foundation::{AppError, TenantId};
use kelson_http_client::{HttpClientPort, HttpRequest, HttpResponse};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

/// One subscribed endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSubscription {
    pub id: Uuid,
    pub url: String,
    /// Event names to deliver (`..` suffix = module wildcard: `order..`).
    pub events: Vec<String>,
    /// HMAC secret; stored by the application via its secret envelope.
    pub secret: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<TenantId>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
}

impl WebhookSubscription {
    pub fn matches_event(&self, event: &str) -> bool {
        self.events.iter().any(|pattern| {
            pattern == event
                || pattern == "*"
                || pattern
                    .strip_suffix("..")
                    .is_some_and(|prefix| event.starts_with(prefix))
        })
    }
}

/// Subscription storage port (in-memory ships; postgres later).
pub trait WebhookStore: Send + Sync {
    fn save(&self, subscription: WebhookSubscription) -> Result<(), AppError>;
    fn delete(&self, id: Uuid) -> Result<(), AppError>;
    fn active_for_event(&self, event: &str) -> Result<Vec<WebhookSubscription>, AppError>;
    fn all(&self) -> Result<Vec<WebhookSubscription>, AppError>;
}

#[derive(Debug, Default)]
pub struct InMemoryWebhookStore {
    inner: RwLock<BTreeMap<Uuid, WebhookSubscription>>,
}

impl InMemoryWebhookStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl WebhookStore for InMemoryWebhookStore {
    fn save(&self, subscription: WebhookSubscription) -> Result<(), AppError> {
        if !subscription.url.starts_with("https://") {
            return Err(AppError::validation("webhook url must be https"));
        }
        self.inner
            .write()
            .unwrap()
            .insert(subscription.id, subscription);
        Ok(())
    }

    fn delete(&self, id: Uuid) -> Result<(), AppError> {
        self.inner.write().unwrap().remove(&id);
        Ok(())
    }

    fn active_for_event(&self, event: &str) -> Result<Vec<WebhookSubscription>, AppError> {
        Ok(self
            .inner
            .read()
            .unwrap()
            .values()
            .filter(|s| s.active && s.matches_event(event))
            .cloned()
            .collect())
    }

    fn all(&self) -> Result<Vec<WebhookSubscription>, AppError> {
        Ok(self.inner.read().unwrap().values().cloned().collect())
    }
}

// ------------------------------------------------------------- signing

const SHA256_BLOCK: usize = 64;

/// HMAC-SHA256 (RFC 2104) over sha2 — small, dep-free implementation.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block_key = [0u8; SHA256_BLOCK];
    if key.len() > SHA256_BLOCK {
        let hashed = Sha256::digest(key);
        block_key[..32].copy_from_slice(&hashed);
    } else {
        block_key[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; SHA256_BLOCK];
    let mut opad = [0x5cu8; SHA256_BLOCK];
    for i in 0..SHA256_BLOCK {
        ipad[i] ^= block_key[i];
        opad[i] ^= block_key[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    outer.finalize().into()
}

pub fn signature_hex(secret: &str, body: &[u8]) -> String {
    let digest = hmac_sha256(secret.as_bytes(), body);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

// ----------------------------------------------------------- dispatcher

pub const SIGNATURE_HEADER: &str = "x-kelson-signature";
pub const EVENT_HEADER: &str = "x-kelson-event";
pub const DELIVERY_HEADER: &str = "x-kelson-delivery";

/// Envelope every webhook receiver gets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookPayload<'a> {
    pub delivery: Uuid,
    pub event: &'a str,
    #[serde(with = "time::serde::iso8601")]
    pub at: OffsetDateTime,
    pub payload: serde_json::Value,
}

/// Outcome of delivering to one subscription.
#[derive(Debug, Clone)]
pub struct DeliveryOutcome {
    pub subscription_id: Uuid,
    pub delivered: bool,
    pub status: Option<u16>,
    pub error: Option<AppError>,
}

pub struct WebhookDispatcher {
    store: Arc<dyn WebhookStore>,
    client: Arc<dyn HttpClientPort>,
}

impl WebhookDispatcher {
    pub fn new(store: Arc<dyn WebhookStore>, client: Arc<dyn HttpClientPort>) -> Self {
        Self { store, client }
    }

    pub fn store(&self) -> &Arc<dyn WebhookStore> {
        &self.store
    }

    /// Deliver `event` to every active matching subscription. Failures are
    /// reported per subscription (never abort the fan-out); callers may
    /// re-enqueue via the jobs layer for later retry.
    pub async fn dispatch(
        &self,
        event: &str,
        payload: serde_json::Value,
        now: OffsetDateTime,
    ) -> Vec<DeliveryOutcome> {
        let subscriptions = match self.store.active_for_event(event) {
            Ok(subs) => subs,
            Err(e) => {
                return vec![DeliveryOutcome {
                    subscription_id: Uuid::nil(),
                    delivered: false,
                    status: None,
                    error: Some(e),
                }]
            }
        };
        let mut outcomes = Vec::new();
        for subscription in subscriptions {
            outcomes.push(
                self.deliver(&subscription, event, payload.clone(), now)
                    .await,
            );
        }
        outcomes
    }

    /// Build + sign + send one delivery.
    pub async fn deliver(
        &self,
        subscription: &WebhookSubscription,
        event: &str,
        payload: serde_json::Value,
        now: OffsetDateTime,
    ) -> DeliveryOutcome {
        let delivery_id = Uuid::now_v7();
        let envelope = WebhookPayload {
            delivery: delivery_id,
            event,
            at: now,
            payload,
        };
        let body = match serde_json::to_vec(&envelope) {
            Ok(bytes) => bytes,
            Err(e) => {
                return DeliveryOutcome {
                    subscription_id: subscription.id,
                    delivered: false,
                    status: None,
                    error: Some(AppError::internal(format!("webhook encode: {e}"))),
                }
            }
        };
        let mut request = HttpRequest {
            method: "POST".into(),
            url: subscription.url.clone(),
            headers: BTreeMap::new(),
            body,
            timeout_ms: None,
        };
        request
            .headers
            .insert("content-type".into(), "application/json".into());
        request
            .headers
            .insert(EVENT_HEADER.into(), event.to_string());
        request
            .headers
            .insert(DELIVERY_HEADER.into(), delivery_id.to_string());
        request.headers.insert(
            SIGNATURE_HEADER.into(),
            format!(
                "sha256={}",
                signature_hex(&subscription.secret, &request.body)
            ),
        );

        match self.client.execute(&request).await {
            Ok(HttpResponse { status, .. }) => DeliveryOutcome {
                subscription_id: subscription.id,
                delivered: (200..300).contains(&status),
                status: Some(status),
                error: None,
            },
            Err(error) => DeliveryOutcome {
                subscription_id: subscription.id,
                delivered: false,
                status: None,
                error: Some(error),
            },
        }
    }
}

/// Convenience for job-handler integration: dispatch and fail the job when
/// any subscription failed (retry budget then applies to the whole fan-out).
pub fn into_result(outcomes: &[DeliveryOutcome]) -> Result<(), AppError> {
    let failed = outcomes.iter().filter(|o| !o.delivered).count();
    if failed == 0 {
        return Ok(());
    }
    let first_error = outcomes
        .iter()
        .find_map(|o| o.error.clone())
        .map(|e| e.public_message)
        .unwrap_or_else(|| "non-2xx response".into());
    Err(AppError::external(
        "webhooks",
        format!("{failed} delivery failure(s): {first_error}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kelson_http_client::MockHttpClient;

    fn subscription(url: &str, events: &[&str]) -> WebhookSubscription {
        WebhookSubscription {
            id: Uuid::now_v7(),
            url: url.into(),
            events: events.iter().map(|s| s.to_string()).collect(),
            secret: "s3cr3t-key".into(),
            active: true,
            tenant_id: None,
            created_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn hmac_matches_rfc4231_test_case_2() {
        // key="Jefe", data="what do ya want for nothing?"
        let digest = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn hmac_long_key_path() {
        // key longer than 64 bytes takes the hash-key path.
        let long_key = vec![0xaa; 131];
        let digest = hmac_sha256(
            &long_key,
            b"Test Using Larger Than Block-Size Key - Hash Key First",
        );
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn event_matching_supports_wildcards() {
        let sub = subscription("https://example.com/hook", &["order..", "billing.paid"]);
        assert!(sub.matches_event("order.created"));
        assert!(sub.matches_event("order.item.added"));
        assert!(sub.matches_event("billing.paid"));
        assert!(!sub.matches_event("billing.failed"));
        let all = subscription("https://example.com/hook", &["*"]);
        assert!(all.matches_event("anything.happened"));
    }

    #[test]
    fn store_requires_https_and_filters_active() {
        let store = InMemoryWebhookStore::new();
        assert!(store
            .save(subscription("http://insecure.example/hook", &["*"]))
            .is_err());
        let mut inactive = subscription("https://example.com/a", &["order.."]);
        inactive.active = false;
        store.save(inactive).unwrap();
        store
            .save(subscription("https://example.com/b", &["order.."]))
            .unwrap();
        let matched = store.active_for_event("order.created").unwrap();
        assert_eq!(matched.len(), 1);
        assert!(matched[0].url.ends_with("/b"));
    }

    #[tokio::test]
    async fn dispatch_signs_and_fans_out() {
        let store = Arc::new(InMemoryWebhookStore::new());
        store
            .save(subscription("https://example.com/one", &["order.."]))
            .unwrap();
        store
            .save(subscription("https://example.com/two", &["billing.."]))
            .unwrap();
        let client = Arc::new(kelson_http_client::MockHttpClient::new());
        client.set_default(kelson_http_client::HttpResponse {
            status: 200,
            headers: Default::default(),
            body: vec![],
        });
        let dispatcher = WebhookDispatcher::new(store.clone(), client.clone());

        let outcomes = dispatcher
            .dispatch(
                "order.created",
                serde_json::json!({"id": 1}),
                OffsetDateTime::now_utc(),
            )
            .await;
        assert_eq!(outcomes.len(), 1, "only the order.. subscription matches");
        assert!(outcomes[0].delivered);

        let sent = &client.calls.lock().unwrap()[0];
        let sig = sent.headers.get(SIGNATURE_HEADER).unwrap();
        assert!(sig.starts_with("sha256="));
        assert_eq!(
            sig.as_str(),
            format!("sha256={}", signature_hex("s3cr3t-key", &sent.body))
        );
        assert_eq!(sent.headers.get(EVENT_HEADER).unwrap(), "order.created");
        let envelope: serde_json::Value = serde_json::from_slice(&sent.body).unwrap();
        assert_eq!(envelope["event"], "order.created");
    }

    #[tokio::test]
    async fn failures_report_per_subscription_and_aggregate() {
        let store = Arc::new(InMemoryWebhookStore::new());
        store
            .save(subscription("https://example.com/ok", &["*"]))
            .unwrap();
        store
            .save(subscription("https://example.com/bad", &["*"]))
            .unwrap();
        let client = Arc::new(MockHttpClient::new());
        client.enqueue(kelson_http_client::HttpResponse {
            status: 200,
            headers: Default::default(),
            body: vec![],
        });
        client.enqueue(kelson_http_client::HttpResponse {
            status: 500,
            headers: Default::default(),
            body: vec![],
        });
        let dispatcher = WebhookDispatcher::new(store, client);
        let outcomes = dispatcher
            .dispatch("x.y", serde_json::json!(null), OffsetDateTime::now_utc())
            .await;
        assert_eq!(outcomes.len(), 2);
        assert!(into_result(&outcomes).is_err());
        let all_ok = outcomes
            .iter()
            .map(|o| DeliveryOutcome {
                delivered: true,
                ..o.clone()
            })
            .collect::<Vec<_>>();
        assert!(into_result(&all_ok).is_ok());
    }
}
