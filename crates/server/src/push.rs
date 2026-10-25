//! Opt-in Web Push: VAPID keys, the browsers that asked to be notified, and
//! sending them an encrypted message (RFC 8291) when a task starts waiting.
//!
//! A message holds the task's title and why it waits, never agent output: that
//! may contain secrets even after redaction, and it would leave the control
//! plane for the browser vendor's push service.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::tenancy::DEFAULT_ORG;

/// The control plane's VAPID identity, from `SIDEFLEET_VAPID_PRIVATE_KEY`.
#[derive(Clone)]
pub struct Vapid {
    key: SigningKey,
    /// The public key browsers subscribe with: an uncompressed P-256 point, base64url.
    pub public_key: String,
    /// Contact for push services (`mailto:` or `https:`).
    pub subject: String,
}

/// Default VAPID contact when `SIDEFLEET_VAPID_SUBJECT` is unset and there is no HTTPS public address.
pub const DEFAULT_SUBJECT: &str = "https://github.com/foodaka/side-fleet";

impl Vapid {
    /// From a base64url P-256 private key (32 bytes), as [`generate_private_key`] writes it.
    pub fn from_private_key(b64: &str, subject: &str) -> anyhow::Result<Self> {
        let bytes = URL_SAFE_NO_PAD.decode(b64.trim().trim_end_matches('='))?;
        let key = SigningKey::from_slice(&bytes).map_err(|_| anyhow::anyhow!("SIDEFLEET_VAPID_PRIVATE_KEY is not a P-256 private key"))?;
        let public_key = URL_SAFE_NO_PAD.encode(key.verifying_key().to_encoded_point(false).as_bytes());
        Ok(Self { key, public_key, subject: subject.to_string() })
    }

    /// `Authorization` header value for a push to `endpoint` (RFC 8292), valid 12 hours.
    pub fn authorization(&self, endpoint: &reqwest::Url) -> String {
        let aud = endpoint.origin().ascii_serialization();
        let exp = chrono::Utc::now().timestamp() + 12 * 3600;
        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = URL_SAFE_NO_PAD.encode(json!({"aud": aud, "exp": exp, "sub": self.subject}).to_string());
        let signing_input = format!("{header}.{claims}");
        let sig: Signature = self.key.sign(signing_input.as_bytes());
        format!("vapid t={signing_input}.{}, k={}", URL_SAFE_NO_PAD.encode(sig.to_bytes()), self.public_key)
    }
}

/// A new VAPID private key, base64url, for `~/.sidefleet/server.env`.
pub fn generate_private_key() -> String {
    URL_SAFE_NO_PAD.encode(SigningKey::random(&mut rand::rngs::OsRng).to_bytes())
}

/// A browser's subscription, as `PushSubscription.toJSON()` gives it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub endpoint: String,
    pub keys: SubscriptionKeys,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionKeys {
    pub p256dh: String,
    pub auth: String,
}

/// Push services of the browsers people use. The control plane only ever
/// sends to these, so a subscription cannot point it at another host.
const PUSH_HOSTS: &[&str] = &[
    "fcm.googleapis.com",
    "android.googleapis.com",
    "push.services.mozilla.com",
    "push.apple.com",
    "notify.windows.com",
];

/// Subscriptions per viewer; the oldest go first.
pub const MAX_PER_VIEWER: i64 = 20;

/// Why a subscription was refused.
pub fn check(sub: &Subscription, any_host: bool) -> Result<(), &'static str> {
    let url = reqwest::Url::parse(&sub.endpoint).map_err(|_| "endpoint is not a URL")?;
    let host = url.host_str().unwrap_or("");
    let known = url.scheme() == "https" && PUSH_HOSTS.iter().any(|h| host == *h || host.ends_with(&format!(".{h}")));
    if !known && !any_host {
        return Err("endpoint is not a known browser push service");
    }
    if sub.endpoint.len() > 2048 {
        return Err("endpoint is too long");
    }
    let key = decode(&sub.keys.p256dh).ok_or("keys.p256dh is not base64url")?;
    p256::PublicKey::from_sec1_bytes(&key).map_err(|_| "keys.p256dh is not a P-256 public key")?;
    if decode(&sub.keys.auth).map(|a| a.len()) != Some(16) {
        return Err("keys.auth must be 16 bytes, base64url");
    }
    Ok(())
}

fn decode(s: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s.trim().trim_end_matches('=')).ok()
}

pub async fn save(pool: &PgPool, viewer: Uuid, sub: &Subscription) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO push_subscriptions (endpoint, viewer_id, p256dh, auth) VALUES ($1, $2, $3, $4)
         ON CONFLICT (endpoint) DO UPDATE SET viewer_id = EXCLUDED.viewer_id, p256dh = EXCLUDED.p256dh, auth = EXCLUDED.auth, created_at = now()",
    )
    .bind(&sub.endpoint)
    .bind(viewer)
    .bind(&sub.keys.p256dh)
    .bind(&sub.keys.auth)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM push_subscriptions WHERE viewer_id = $1 AND endpoint NOT IN
           (SELECT endpoint FROM push_subscriptions WHERE viewer_id = $1 ORDER BY created_at DESC LIMIT $2)",
    )
    .bind(viewer)
    .bind(MAX_PER_VIEWER)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Forget a browser. Only its own viewer can remove it; returns whether it existed.
pub async fn delete(pool: &PgPool, viewer: Uuid, endpoint: &str) -> sqlx::Result<bool> {
    let r = sqlx::query("DELETE FROM push_subscriptions WHERE endpoint = $1 AND viewer_id = $2").bind(endpoint).bind(viewer).execute(pool).await?;
    Ok(r.rows_affected() > 0)
}

/// Browsers of everyone who can see `root` (in `org`) and has not looked at it
/// since `since`: org members, and the developer key's viewer in the default org.
pub async fn recipients(pool: &PgPool, org: Uuid, root: Uuid, since: chrono::DateTime<chrono::Utc>) -> sqlx::Result<Vec<Subscription>> {
    let rows = sqlx::query(
        "SELECT p.endpoint, p.p256dh, p.auth FROM push_subscriptions p
         WHERE (p.viewer_id IN (SELECT user_id FROM org_members WHERE org_id = $1) OR (p.viewer_id = $4 AND $1 = $5))
           AND NOT EXISTS (SELECT 1 FROM inbox_seen s WHERE s.viewer_id = p.viewer_id AND s.root_id = $2 AND s.seen_at >= $3)
           AND NOT EXISTS (SELECT 1 FROM inbox_cleared c WHERE c.viewer_id = p.viewer_id AND c.org_id = $1 AND c.cleared_at >= $3)",
    )
    .bind(org)
    .bind(root)
    .bind(since)
    .bind(Uuid::nil())
    .bind(DEFAULT_ORG)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| Subscription { endpoint: r.get("endpoint"), keys: SubscriptionKeys { p256dh: r.get("p256dh"), auth: r.get("auth") } })
        .collect())
}

pub async fn forget_endpoint(pool: &PgPool, endpoint: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM push_subscriptions WHERE endpoint = $1").bind(endpoint).execute(pool).await?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Sent,
    /// The browser unsubscribed or the subscription expired: forget it.
    Gone,
    Failed(String),
}

/// How long a push service keeps trying to reach an offline browser.
const TTL_SECS: u64 = 24 * 3600;

/// Encrypt `payload` for one browser and hand it to its push service.
pub async fn send(http: &reqwest::Client, vapid: &Vapid, sub: &Subscription, payload: &[u8]) -> Delivery {
    let Ok(url) = reqwest::Url::parse(&sub.endpoint) else { return Delivery::Gone };
    let (Some(key), Some(auth)) = (decode(&sub.keys.p256dh), decode(&sub.keys.auth)) else { return Delivery::Gone };
    let (Ok(key), true) = (p256::PublicKey::from_sec1_bytes(&key), auth.len() == 16) else { return Delivery::Gone };
    let auth = web_push_native::Auth::clone_from_slice(&auth);
    let body = match web_push_native::encrypt(payload.to_vec(), &key, &auth) {
        Ok(b) => b,
        Err(e) => return Delivery::Failed(format!("encrypt: {e}")),
    };
    let r = http
        .post(url.clone())
        .header("TTL", TTL_SECS.to_string())
        .header("Urgency", "normal")
        .header(reqwest::header::CONTENT_ENCODING, "aes128gcm")
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(reqwest::header::AUTHORIZATION, vapid.authorization(&url))
        .body(body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;
    match r {
        Ok(r) if r.status().is_success() => Delivery::Sent,
        Ok(r) if matches!(r.status().as_u16(), 404 | 410) => Delivery::Gone,
        Ok(r) => Delivery::Failed(format!("push service answered {}", r.status())),
        Err(e) => Delivery::Failed(e.without_url().to_string()),
    }
}

/// What a notification says: the task's title and why it waits, nothing else.
pub fn payload(root: Uuid, title: &str, reason: crate::attention::Reason) -> Vec<u8> {
    json!({
        "id": root,
        "title": clip(title, 120),
        "reason": reason,
        "label": reason.label(),
        "url": format!("/#/runs/{root}"),
    })
    .to_string()
    .into_bytes()
}

/// The first line of `s`, at most `max` characters, with an ellipsis when cut.
fn clip(s: &str, max: usize) -> String {
    let line = s.trim().lines().next().unwrap_or("").trim();
    if line.chars().count() <= max && !s.trim().contains('\n') {
        return line.to_string();
    }
    let mut out: String = line.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::elliptic_curve::sec1::ToEncodedPoint;

    fn browser() -> (p256::SecretKey, Subscription, [u8; 16]) {
        let secret = p256::SecretKey::random(&mut rand::rngs::OsRng);
        let auth: [u8; 16] = rand::random();
        let sub = Subscription {
            endpoint: "https://fcm.googleapis.com/fcm/send/abc".into(),
            keys: SubscriptionKeys {
                p256dh: URL_SAFE_NO_PAD.encode(secret.public_key().to_encoded_point(false).as_bytes()),
                auth: URL_SAFE_NO_PAD.encode(auth),
            },
        };
        (secret, sub, auth)
    }

    #[test]
    fn generated_keys_load_and_sign_a_vapid_header() {
        use p256::ecdsa::{signature::Verifier, VerifyingKey};
        let v = Vapid::from_private_key(&generate_private_key(), DEFAULT_SUBJECT).unwrap();
        assert_eq!(URL_SAFE_NO_PAD.decode(&v.public_key).unwrap().len(), 65);
        let h = v.authorization(&reqwest::Url::parse("https://fcm.googleapis.com:443/fcm/send/x").unwrap());
        let (t, k) = h.strip_prefix("vapid t=").unwrap().split_once(", k=").unwrap();
        assert_eq!(k, v.public_key);
        let parts: Vec<&str> = t.split('.').collect();
        let claims: serde_json::Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        assert_eq!(claims["sub"], DEFAULT_SUBJECT);
        let key = VerifyingKey::from_sec1_bytes(&URL_SAFE_NO_PAD.decode(k).unwrap()).unwrap();
        let sig = Signature::from_slice(&URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).unwrap();
        key.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig).expect("ES256 signature verifies");
    }

    #[test]
    fn only_browser_push_services_are_accepted() {
        let (_, mut sub, _) = browser();
        assert_eq!(check(&sub, false), Ok(()));
        for ok in ["https://updates.push.services.mozilla.com/wpush/v2/x", "https://web.push.apple.com/x", "https://wns2-par02p.notify.windows.com/w/?token=x"] {
            sub.endpoint = ok.into();
            assert_eq!(check(&sub, false), Ok(()), "{ok}");
        }
        for bad in ["http://fcm.googleapis.com/x", "https://169.254.169.254/latest", "https://evilfcm.googleapis.com.example/x", "https://localhost:7373/v1/runs"] {
            sub.endpoint = bad.into();
            assert!(check(&sub, false).is_err(), "{bad}");
        }
        sub.endpoint = "http://127.0.0.1:9/push".into();
        assert_eq!(check(&sub, true), Ok(()));
        sub.keys.auth = "c2hvcnQ".into();
        assert!(check(&sub, true).is_err());
    }

    #[test]
    fn the_message_only_says_title_and_reason_and_decrypts_in_the_browser() {
        let (secret, sub, auth) = browser();
        let root = Uuid::new_v4();
        let msg = payload(root, "Fix the login bug\nwith this token: abc", crate::attention::Reason::Failed);
        let v: serde_json::Value = serde_json::from_slice(&msg).unwrap();
        assert_eq!(v["title"], "Fix the login bug…");
        assert_eq!(v["reason"], "failed");
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["id", "label", "reason", "title", "url"]);
        let key = p256::PublicKey::from_sec1_bytes(&decode(&sub.keys.p256dh).unwrap()).unwrap();
        let sealed = web_push_native::encrypt(msg.clone(), &key, web_push_native::Auth::from_slice(&auth)).unwrap();
        let opened = web_push_native::decrypt(sealed, &secret, web_push_native::Auth::from_slice(&auth)).unwrap();
        assert_eq!(opened, msg);
    }
}
