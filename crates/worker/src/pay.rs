//! Agent budgets: the agent pays for APIs over MPP (HTTP 402), from a budget.
//!
//! For a run with a budget the worker makes a fresh access key, in memory only,
//! and offers its address to the control plane ([`authorize`]). The user's root
//! key, which never comes near this machine, signs an authorization for it with
//! an expiry and a spending limit (`sidefleet wallet authorize`); Tempo itself
//! refuses anything past that limit, whatever happens here.
//!
//! The agent gets one MCP tool, `pay` (`sidefleet pay-mcp --socket <path>`, the
//! same shape as "Ask me"'s prompt tool): it asks for a URL, and the worker
//! ([`Payer`]) makes the request. When the service answers 402 the worker checks
//! the price against what is left of the budget, asks on the page above the
//! run's threshold, for a service that isn't one of the user's approved ones, or
//! when a service asks to be paid to another address than before, pays with the
//! key, and returns the paid response. The agent never sees the key.

use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alloy::primitives::{Address, U256};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use mpp::client::tempo::signing::{KeychainVersion, TempoSigningMode};
use mpp::client::{PaymentProvider, TempoProvider};
use mpp::{ChargeRequest, PaymentChallenge, PrivateKeySigner};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sidefleet_core::{PayKeyOffer, PayMode, PayPolicy, PaymentReport, Run, PAY_TOOL_NAME};
use tempo_alloy::primitives::transaction::SignedKeyAuthorization;
use tokio::sync::watch;
use uuid::Uuid;

use crate::approval::{self, Ask, Decision};
use crate::client::ControlClient;

/// Name of the MCP server in `--mcp-config`, and the tool Claude Code calls.
pub const MCP_SERVER: &str = "sidefleet_pay";
const TOOL: &str = "pay";
pub const PAY_TOOL: &str = "mcp__sidefleet_pay__pay";
/// Tempo's testnet, which is all this spike pays on.
pub const TESTNET_RPC: &str = "https://rpc.moderato.tempo.xyz";
pub const TESTNET_CHAIN_ID: u64 = 42431;
/// pathUSD, the testnet's dollar stablecoin (6 decimals: base units are micros).
pub const PATH_USD: &str = "0x20c0000000000000000000000000000000000000";
/// A run's key stops working this long after it is made, or when the run ends.
pub const KEY_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
/// The most of a paid response the agent gets back.
const MAX_BODY_BYTES: usize = 64 * 1024;

/// Where this machine pays: set with `SIDEFLEET_PAYMENTS=1` (and optionally
/// `SIDEFLEET_TEMPO_RPC`, `SIDEFLEET_TEMPO_CHAIN_ID`, `SIDEFLEET_PAY_TOKEN`).
#[derive(Debug, Clone)]
pub struct PaymentsConfig {
    pub rpc_url: String,
    pub chain_id: u64,
    /// The TIP-20 token budgets are in.
    pub token: String,
    pub keychain: KeychainVersion,
}

impl PaymentsConfig {
    /// From the environment; None unless payments are turned on for this machine.
    pub fn from_env() -> Option<PaymentsConfig> {
        let on = std::env::var("SIDEFLEET_PAYMENTS").is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes"));
        if !on {
            return None;
        }
        let var = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        Some(PaymentsConfig {
            rpc_url: var("SIDEFLEET_TEMPO_RPC").unwrap_or_else(|| TESTNET_RPC.into()),
            chain_id: var("SIDEFLEET_TEMPO_CHAIN_ID").and_then(|c| c.parse().ok()).unwrap_or(TESTNET_CHAIN_ID),
            token: var("SIDEFLEET_PAY_TOKEN").unwrap_or_else(|| PATH_USD.into()),
            // V2 binds the signature to the account; V1 only for a chain before the T1C upgrade.
            keychain: if var("SIDEFLEET_TEMPO_KEYCHAIN_V1").is_some() { KeychainVersion::V1 } else { KeychainVersion::V2 },
        })
    }
}

/// `$1.20` for 1_200_000 micros.
pub fn usd(micros: i64) -> String {
    let cents = (micros as f64 / 10_000.0).round() / 100.0;
    if micros != 0 && micros.abs() < 10_000 {
        format!("${:.6}", micros as f64 / 1_000_000.0).trim_end_matches('0').to_string()
    } else {
        format!("${cents:.2}")
    }
}

/// The socket for a run's payments, beside "Ask me"'s.
pub fn socket_dir(run_id: Uuid) -> PathBuf {
    std::env::temp_dir().join(format!("sf-pay-{}", &run_id.simple().to_string()[..16]))
}

/// The `mcpServers` entry that makes Claude Code spawn `<exe> pay-mcp --socket <socket>`.
pub fn mcp_server(exe: &Path, socket: &Path) -> Value {
    json!({
        "type": "stdio",
        "command": exe.to_string_lossy(),
        "args": ["pay-mcp", "--socket", socket.to_string_lossy()],
    })
}

/// Told to the agent with its task, so it knows the tool and its budget.
pub fn agent_hint(budget_micros: i64, ask_above_micros: i64) -> String {
    format!(
        "You have a budget of {} for paid APIs in this task. When a service needs payment (HTTP 402, \
         the Machine Payments Protocol), call the `{PAY_TOOL}` tool with the URL instead of fetching it \
         yourself: it pays from the budget and returns the response. Payments over {} wait for the user \
         to approve them. Say what each payment is for in `reason`, and don't spend more than the task needs.",
        usd(budget_micros),
        usd(ask_above_micros)
    )
}

// ---- making and authorizing the run's key ----

/// A run's key, authorized by the user: what [`Payer`] pays with.
pub struct AuthorizedKey {
    signer: PrivateKeySigner,
    wallet: Address,
    authorization: SignedKeyAuthorization,
    offer: PayKeyOffer,
}

/// Make a key for `run`, offer it, and wait up to `timeout` for the user to authorize it.
/// None when they declined, nobody did in time, or the run stopped.
pub async fn authorize(
    client: &ControlClient,
    cfg: &PaymentsConfig,
    run: &Run,
    timeout: Duration,
    mut stop: watch::Receiver<bool>,
) -> Result<Option<AuthorizedKey>> {
    let signer = PrivateKeySigner::random();
    let expires_at = Utc::now() + chrono::Duration::from_std(KEY_LIFETIME)?;
    let offer = PayKeyOffer {
        address: signer.address().to_checksum(None),
        chain_id: cfg.chain_id,
        token: cfg.token.clone(),
        expires_at,
        timeout_secs: Some(timeout.as_secs().max(1)),
    };
    client.offer_pay_key(run.id, &offer).await?;
    tracing::info!(run_id = %run.id, key = %offer.address, "waiting for the user to authorize the run's budget");
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match client.pay_key_status(run.id).await {
            Ok(st) => {
                if st.declined {
                    return Ok(None);
                }
                if let (Some(wallet), Some(auth)) = (st.wallet, st.authorization) {
                    let authorization = check_authorization(&offer, &wallet, auth, run.budget_micros.unwrap_or(0))?;
                    let wallet = wallet.parse::<Address>().context("the authorization's wallet is not an address")?;
                    return Ok(Some(AuthorizedKey { signer, wallet, authorization, offer }));
                }
            }
            Err(e) => tracing::debug!(error = %format!("{e:#}"), "payment key poll failed"),
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(None);
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    return Ok(None);
                }
            }
        }
    }
}

/// The authorization must be for this key, on this chain, signed by `wallet`, and limit
/// spending in the budget's token to at most the budget: anything else is refused before use.
fn check_authorization(offer: &PayKeyOffer, wallet: &str, auth: Value, budget: i64) -> Result<SignedKeyAuthorization> {
    let auth: SignedKeyAuthorization = match auth {
        // From the page: viem hands over the signed authorization RLP-encoded, as it goes in a transaction.
        Value::String(hex) => {
            let bytes = alloy::hex::decode(hex.trim()).context("the authorization is not hex")?;
            alloy::rlp::Decodable::decode(&mut bytes.as_slice()).map_err(|e| anyhow::anyhow!("the authorization is not a signed key authorization: {e}"))?
        }
        // From `sidefleet wallet authorize`.
        v => serde_json::from_value(v).context("the authorization is not a signed key authorization")?,
    };
    let key: Address = offer.address.parse()?;
    if auth.key_id != key {
        bail!("the authorization is for another key ({})", auth.key_id);
    }
    if auth.chain_id != offer.chain_id {
        bail!("the authorization is for chain {}, not {}", auth.chain_id, offer.chain_id);
    }
    let signer = auth.recover_signer().map_err(|e| anyhow::anyhow!("the authorization's signature is invalid: {e}"))?;
    if !signer.to_string().eq_ignore_ascii_case(wallet) {
        bail!("the authorization was signed by {signer}, not the wallet {wallet}");
    }
    let token: Address = offer.token.parse()?;
    let limit = auth.limits.as_ref().and_then(|ls| ls.iter().find(|l| l.token == token)).map(|l| l.limit);
    match limit {
        Some(l) if l <= U256::from(budget.max(0) as u64) => Ok(auth),
        Some(l) => bail!("the authorization allows {l} base units, more than the budget"),
        None => bail!("the authorization sets no spending limit in {}", offer.token),
    }
}

// ---- paying ----

/// What the agent asks for: a request to a service that may want payment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayRequest {
    pub url: String,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
    /// What the payment is for, shown to the user.
    #[serde(default)]
    pub reason: Option<String>,
}

struct Spend {
    spent: i64,
    /// Sent with the first payment, which provisions the key on chain; dropped after.
    provisioning: bool,
    /// Services the user approved paying during this run, and the address they approved.
    approved_here: std::collections::HashMap<String, Option<String>>,
}

/// Pays for a run's requests from its authorized key, one at a time.
pub struct Payer {
    client: ControlClient,
    run_id: Uuid,
    budget: i64,
    ask_above: i64,
    /// The user's approved services, as they were when the run was created.
    policy: PayPolicy,
    cfg: PaymentsConfig,
    key: AuthorizedKey,
    approval_timeout: Duration,
    stop: watch::Receiver<bool>,
    http: reqwest::Client,
    spend: tokio::sync::Mutex<Spend>,
}

impl Payer {
    pub fn new(client: ControlClient, run: &Run, cfg: PaymentsConfig, key: AuthorizedKey, approval_timeout: Duration, stop: watch::Receiver<bool>) -> Result<Payer> {
        let http = reqwest::Client::builder()
            // A credential never follows a redirect to another host.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Payer {
            client,
            run_id: run.id,
            budget: run.budget_micros.unwrap_or(0),
            ask_above: run.ask_above_micros.unwrap_or(sidefleet_core::DEFAULT_ASK_ABOVE_MICROS),
            policy: run.pay_policy.clone().unwrap_or_default(),
            cfg,
            key,
            approval_timeout,
            stop,
            http,
            spend: tokio::sync::Mutex::new(Spend { spent: run.spent_micros, provisioning: true, approved_here: Default::default() }),
        })
    }

    pub fn key_address(&self) -> &str {
        &self.key.offer.address
    }

    fn provider(&self, provisioning: bool, max: i64) -> Result<TempoProvider> {
        let mode = TempoSigningMode::Keychain {
            wallet: self.key.wallet,
            key_authorization: provisioning.then(|| Box::new(self.key.authorization.clone())),
            version: self.cfg.keychain,
        };
        Ok(TempoProvider::new(self.key.signer.clone(), &self.cfg.rpc_url)?
            .with_signing_mode(mode)
            .with_expected_chain_id(self.cfg.chain_id)
            .with_currencies([self.cfg.token.clone()])
            .with_max_atomic_amount(U256::from(max.max(0) as u64)))
    }

    fn request(&self, req: &PayRequest, credential: Option<&str>) -> Result<reqwest::RequestBuilder> {
        let method = reqwest::Method::from_bytes(req.method.as_deref().unwrap_or("GET").to_ascii_uppercase().as_bytes()).context("invalid method")?;
        let mut b = self.http.request(method, &req.url).header("Accept-Payment", "tempo/charge");
        for (k, v) in &req.headers {
            if k.eq_ignore_ascii_case("authorization") || k.eq_ignore_ascii_case("host") {
                continue;
            }
            b = b.header(k, v);
        }
        if let Some(body) = &req.body {
            b = b.body(body.clone());
        }
        if let Some(c) = credential {
            b = b.header(mpp::AUTHORIZATION_HEADER, c);
        }
        Ok(b)
    }

    async fn report(&self, report: PaymentReport) -> Option<i64> {
        match self.client.record_payment(self.run_id, &report).await {
            Ok(r) => Some(r.spent_micros),
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), run_id = %self.run_id, "reporting a payment failed");
                None
            }
        }
    }

    /// Make the agent's request, paying for it if the service asks. The answer is the
    /// tool's result for the agent: the response, or why nothing was paid.
    pub async fn pay(&self, req: PayRequest) -> Value {
        match self.try_pay(&req).await {
            Ok(v) => v,
            Err(e) => json!({"error": format!("{e:#}")}),
        }
    }

    async fn try_pay(&self, req: &PayRequest) -> Result<Value> {
        let url = reqwest::Url::parse(&req.url).context("invalid url")?;
        if url.scheme() != "https" {
            bail!("only https URLs can be paid for");
        }
        let host = url.host_str().unwrap_or_default().to_string();
        // One payment at a time, so two can't both fit in what is left.
        let mut spend = self.spend.lock().await;
        let first = self.request(req, None)?.send().await.context("request failed")?;
        if first.status() != reqwest::StatusCode::PAYMENT_REQUIRED {
            return response_json(first, None, spend.spent, self.budget).await;
        }
        let challenge = first
            .headers()
            .get_all(mpp::WWW_AUTHENTICATE_HEADER)
            .iter()
            .filter_map(|h| h.to_str().ok())
            .filter_map(|h| mpp::parse_www_authenticate(h).ok())
            .find(|c| c.method.eq_ignore_ascii_case("tempo") && c.intent.as_str().eq_ignore_ascii_case("charge"))
            .context("the service wants payment, but not a Tempo charge this tool can pay")?;
        let charge: ChargeRequest = challenge.request.decode().context("reading the price")?;
        if !charge.currency.eq_ignore_ascii_case(&self.cfg.token) {
            bail!("the service wants {}, but the budget is in {}", charge.currency, self.cfg.token);
        }
        let amount: i64 = charge.amount.parse().context("reading the price")?;
        let left = self.budget - spend.spent;
        let base = PaymentReport {
            outcome: String::new(),
            amount_micros: amount,
            url: req.url.clone(),
            host: host.clone(),
            reason: req.reason.clone(),
            detail: None,
            receipt: None,
            recipient: charge.recipient.clone(),
        };
        if amount > left {
            let detail = format!("{} is more than the {} left of the {} budget", usd(amount), usd(left), usd(self.budget));
            self.report(PaymentReport { outcome: "refused".into(), detail: Some(detail.clone()), ..base }).await;
            bail!("not paid: {detail}");
        }
        let recipient = charge.recipient.clone();
        let asks = why_ask(&self.policy, &spend.approved_here, self.ask_above, &host, recipient.as_deref(), amount);
        if !asks.is_empty() {
            let listed = self.policy.find(&host);
            let ask = Ask {
                tool_name: PAY_TOOL_NAME.into(),
                input: json!({
                    "amount_micros": amount,
                    "amount": usd(amount),
                    "host": host,
                    "service": listed.and_then(|s| s.name.clone()),
                    "url": req.url,
                    "reason": req.reason,
                    "recipient": recipient,
                    "asks": asks,
                    "unlisted": listed.is_none(),
                    "left_after_micros": left - amount,
                    "budget_micros": self.budget,
                }),
                tool_use_id: None,
            };
            if let Decision::Deny(why) = approval::ask_page(&self.client, self.run_id, &ask, self.approval_timeout, self.stop.clone()).await {
                self.report(PaymentReport { outcome: "refused".into(), detail: Some(why.clone()), ..base }).await;
                bail!("not paid: {why}");
            }
            spend.approved_here.insert(host.clone(), recipient);
        }
        let paid = self.pay_challenge(req, &challenge, spend.provisioning, left).await;
        match paid {
            Ok((resp, receipt)) => {
                spend.provisioning = false;
                spend.spent += amount;
                let receipt = receipt.and_then(|r| serde_json::to_value(r).ok());
                if let Some(s) = self.report(PaymentReport { outcome: "paid".into(), receipt: receipt.clone(), ..base }).await {
                    spend.spent = s;
                }
                response_json(resp, Some((amount, receipt)), spend.spent, self.budget).await
            }
            Err(e) => {
                self.report(PaymentReport { outcome: "failed".into(), detail: Some(format!("{e:#}")), ..base }).await;
                Err(e)
            }
        }
    }

    async fn pay_challenge(&self, req: &PayRequest, challenge: &PaymentChallenge, provisioning: bool, left: i64) -> Result<(reqwest::Response, Option<mpp::Receipt>)> {
        let credential = self.provider(provisioning, left)?.pay(challenge).await.context("signing the payment")?;
        let header = mpp::format_authorization(&credential).context("encoding the payment")?;
        let resp = self.request(req, Some(&header))?.send().await.context("sending the payment")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body: String = resp.text().await.unwrap_or_default().chars().take(500).collect();
            bail!("the service did not accept the payment ({status}): {body}");
        }
        let receipt = resp.headers().get(mpp::PAYMENT_RECEIPT_HEADER).and_then(|h| h.to_str().ok()).and_then(|h| mpp::parse_receipt(h).ok());
        Ok((resp, receipt))
    }
}

/// Why a payment needs the user's OK, if it does: a service that isn't approved (unless any
/// may be paid, or it was approved earlier in the run), an address other than the one
/// approved or first paid, or an amount over the threshold.
fn why_ask(
    policy: &PayPolicy,
    approved_here: &std::collections::HashMap<String, Option<String>>,
    ask_above: i64,
    host: &str,
    recipient: Option<&str>,
    amount: i64,
) -> Vec<String> {
    let listed = policy.find(host);
    let here = approved_here.get(host);
    let mut asks = Vec::new();
    if policy.mode == PayMode::Approved && listed.is_none() && here.is_none() {
        asks.push(format!("{host} isn't one of your approved services"));
    }
    let pinned = here.cloned().flatten().or_else(|| listed.and_then(|s| s.recipient.clone()));
    if let (Some(p), Some(r)) = (pinned, recipient) {
        if !p.eq_ignore_ascii_case(r) {
            asks.push(format!("it asks to be paid to {r}, not {p} as before"));
        }
    }
    if amount > ask_above {
        asks.push(format!("{} is over your {} threshold", usd(amount), usd(ask_above)));
    }
    asks
}

/// The response as the agent gets it: status, type and (clipped) body, plus what was paid.
async fn response_json(resp: reqwest::Response, paid: Option<(i64, Option<Value>)>, spent: i64, budget: i64) -> Result<Value> {
    let status = resp.status().as_u16();
    let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string);
    let bytes = resp.bytes().await.context("reading the response")?;
    let clipped = bytes.len() > MAX_BODY_BYTES;
    let body = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_BODY_BYTES)]).to_string();
    let mut v = json!({"status": status, "content_type": content_type, "body": body, "body_clipped": clipped});
    if let Some((amount, receipt)) = paid {
        v["paid"] = json!({"amount": usd(amount), "receipt": receipt});
    }
    v["budget_left"] = json!(usd(budget - spent));
    Ok(v)
}

// ---- the MCP server Claude Code spawns ----

fn relay(socket: &Path, args: &Value) -> Value {
    approval::ask_socket(socket, args).unwrap_or_else(|e| json!({"error": format!("Sidefleet could not reach its worker to pay ({e})")}))
}

/// `sidefleet pay-mcp --socket <path>`: serve the pay tool on stdin/stdout.
pub fn serve_mcp_stdio(socket: &Path) -> std::io::Result<()> {
    let tool = json!({
        "name": TOOL,
        "description": "Make an HTTPS request to a service that may charge for it (HTTP 402, the Machine \
            Payments Protocol). Pays from this task's budget, asking the user first above a threshold, and \
            returns the service's response with what was paid.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "The https URL to request."},
                "method": {"type": "string", "description": "GET (default), POST, ..."},
                "headers": {"type": "object", "additionalProperties": {"type": "string"}},
                "body": {"type": "string"},
                "reason": {"type": "string", "description": "What the payment is for, shown to the user."},
            },
            "required": ["url", "reason"],
        },
    });
    approval::serve_tool(std::io::stdin().lock(), Arc::new(Mutex::new(std::io::stdout())), MCP_SERVER, tool, socket, relay)
}

/// When the key expires, for the thread.
pub fn expires_at(key: &AuthorizedKey) -> DateTime<Utc> {
    key.offer.expires_at
}

/// Proves a key can sign for an authorization: used by `sidefleet wallet authorize`.
pub fn sign_authorization(root: &PrivateKeySigner, offer: &PayKeyOffer, budget_micros: i64) -> Result<SignedKeyAuthorization> {
    use alloy::signers::SignerSync;
    use tempo_alloy::primitives::transaction::{KeyAuthorization, PrimitiveSignature, SignatureType, TokenLimit};
    let auth = KeyAuthorization {
        chain_id: offer.chain_id,
        key_type: SignatureType::Secp256k1,
        key_id: offer.address.parse().context("the key is not an address")?,
        expiry: NonZeroU64::new(offer.expires_at.timestamp().max(1) as u64),
        limits: Some(vec![TokenLimit { token: offer.token.parse().context("the token is not an address")?, limit: U256::from(budget_micros.max(0) as u64), period: 0 }]),
        allowed_calls: None,
        witness: None,
        is_admin: false,
        account: None,
    };
    let sig = root.sign_hash_sync(&auth.signature_hash())?;
    Ok(auth.into_signed(PrimitiveSignature::Secp256k1(sig)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(key: &PrivateKeySigner) -> PayKeyOffer {
        PayKeyOffer { address: key.address().to_checksum(None), chain_id: TESTNET_CHAIN_ID, token: PATH_USD.into(), expires_at: Utc::now(), timeout_secs: None }
    }

    #[test]
    fn formats_dollars() {
        assert_eq!(usd(1_200_000), "$1.20");
        assert_eq!(usd(100_000), "$0.10");
        assert_eq!(usd(5_000_000), "$5.00");
        assert_eq!(usd(0), "$0.00");
        assert_eq!(usd(1_500), "$0.0015");
    }

    #[test]
    fn accepts_only_an_authorization_for_this_key_within_the_budget() {
        let (root, key) = (PrivateKeySigner::random(), PrivateKeySigner::random());
        let o = offer(&key);
        let wallet = root.address().to_string();
        let good = serde_json::to_value(sign_authorization(&root, &o, 5_000_000).unwrap()).unwrap();
        assert!(good.get("keyId").is_some(), "the server looks for keyId: {good}");
        assert!(check_authorization(&o, &wallet, good.clone(), 5_000_000).is_ok());
        // Signed by someone else than the wallet it claims.
        assert!(check_authorization(&o, &PrivateKeySigner::random().address().to_string(), good.clone(), 5_000_000).is_err());
        // Allows more than the budget.
        assert!(check_authorization(&o, &wallet, good, 1_000_000).is_err());
        // For another key.
        let other = serde_json::to_value(sign_authorization(&root, &offer(&PrivateKeySigner::random()), 5_000_000).unwrap()).unwrap();
        assert!(check_authorization(&o, &wallet, other, 5_000_000).is_err());
    }

    #[test]
    fn asks_for_unapproved_services_changed_addresses_and_big_amounts() {
        use sidefleet_core::ApprovedService;
        let (a, b) = ("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        let policy = PayPolicy {
            mode: PayMode::Approved,
            approved: vec![
                ApprovedService { host: "api.dune.com".into(), name: Some("Dune".into()), recipient: Some(a.into()) },
                ApprovedService { host: "api.exa.ai".into(), name: None, recipient: None },
            ],
        };
        let none = Default::default();
        let ask = |p: &PayPolicy, here, host, to, amount| why_ask(p, here, 1_000_000, host, to, amount);
        assert!(ask(&policy, &none, "api.dune.com", Some(a), 100_000).is_empty(), "approved, same address, small");
        assert!(ask(&policy, &none, "API.Dune.com", Some(a), 100_000).is_empty(), "hosts match in any case");
        assert!(ask(&policy, &none, "api.exa.ai", Some(b), 100_000).is_empty(), "approved, nothing pinned yet");
        assert_eq!(ask(&policy, &none, "api.dune.com", Some(b), 100_000).len(), 1, "another address than before");
        assert_eq!(ask(&policy, &none, "evil.example", Some(b), 100_000).len(), 1, "not approved");
        assert_eq!(ask(&policy, &none, "evil.example", Some(b), 2_000_000).len(), 2, "not approved and over the threshold");
        let any = PayPolicy { mode: PayMode::Any, ..policy.clone() };
        assert!(ask(&any, &none, "evil.example", Some(b), 100_000).is_empty(), "any service may be paid");
        assert_eq!(ask(&any, &none, "api.dune.com", Some(b), 100_000).len(), 1, "a pinned address still holds");
        // Approved during the run: the next payment to it goes through, to the same address.
        let here = std::collections::HashMap::from([("evil.example".to_string(), Some(b.to_string()))]);
        assert!(ask(&policy, &here, "evil.example", Some(b), 100_000).is_empty());
        assert_eq!(ask(&policy, &here, "evil.example", Some(a), 100_000).len(), 1);
    }

    #[test]
    fn accepts_a_passkey_signed_authorization_from_the_page() {
        // Signed by viem with a WebAuthn (passkey) root account, as the page does: $1 for this key.
        let wallet = "0x732691c6Db33f04Be30353eb6eFd24867deF78c5";
        let o = PayKeyOffer { address: "0xF497eb279634e0497a8eAdF27B677aBA748b2960".into(), chain_id: TESTNET_CHAIN_ID, token: PATH_USD.into(), expires_at: Utc::now(), timeout_secs: None };
        let auth = json!("0xf90165f83982a5bf8094f497eb279634e0497a8eadf27b677aba748b2960846ae3089cdad99420c0000000000000000000000000000000000000830f4240b901270249960de5880e8c687434170f6476605b8fe4aeb9a28632c7995cf3ba831d976305000000007b2274797065223a22776562617574686e2e676574222c226368616c6c656e6765223a22656e54474838665349623165576d6866457042632d70687335634a4a5f43377357476634344e664c485355222c226f726967696e223a22687474703a2f2f6c6f63616c686f7374222c2263726f73734f726967696e223a66616c73657da0f5f20f671554cd5f90051d43c65b358cbca6b5cbf56d12c946eb225e0ae6bb3827fdf392ad54c6c843557fdf6ea639dee4af0fe280bb442547a1ed5000ff214b51325b53317639209bc3583e79bb74a83f41896a0e8b4e1629f05dc04777ba15a9e4368fd23ad83cb2bd76954097e8c2dff67acf362f13587509b77696e3dd");
        let signed = check_authorization(&o, wallet, auth.clone(), 1_000_000).unwrap();
        assert_eq!(signed.recover_signer().unwrap().to_string(), wallet);
        assert!(check_authorization(&o, wallet, auth.clone(), 500_000).is_err(), "more than the budget");
        assert!(check_authorization(&o, "0x2222222222222222222222222222222222222222", auth, 1_000_000).is_err(), "another wallet");
    }

    /// A passkey wallet's authorization works on chain: its access key pays a $0.10 charge.
    /// Needs the testnet and `SIDEFLEET_TEST_PASSKEY_AUTH`, a JSON file with `wallet`,
    /// `key_private`, `key_address` and `authorization` (signed by viem, as the page does);
    /// the wallet is funded from the faucet first. Run with `--ignored`.
    #[tokio::test]
    #[ignore]
    async fn a_passkey_wallet_pays_through_its_access_key() {
        let f: Value = serde_json::from_str(&std::fs::read_to_string(std::env::var("SIDEFLEET_TEST_PASSKEY_AUTH").unwrap()).unwrap()).unwrap();
        let wallet = f["wallet"].as_str().unwrap();
        crate::wallet::fund(TESTNET_RPC, wallet).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let o = PayKeyOffer { address: f["key_address"].as_str().unwrap().into(), chain_id: TESTNET_CHAIN_ID, token: PATH_USD.into(), expires_at: Utc::now(), timeout_secs: None };
        let auth = check_authorization(&o, wallet, f["authorization"].clone(), 1_000_000).unwrap();
        let key: PrivateKeySigner = f["key_private"].as_str().unwrap().parse().unwrap();
        let provider = TempoProvider::new(key, TESTNET_RPC)
            .unwrap()
            .with_signing_mode(TempoSigningMode::Keychain { wallet: wallet.parse().unwrap(), key_authorization: Some(Box::new(auth)), version: KeychainVersion::V2 })
            .with_expected_chain_id(TESTNET_CHAIN_ID);
        let url = "https://mpp.dev/api/ping/paid";
        let first = reqwest::get(url).await.unwrap();
        let challenge = mpp::parse_www_authenticate(first.headers()[mpp::WWW_AUTHENTICATE_HEADER].to_str().unwrap()).unwrap();
        let credential = provider.pay(&challenge).await.unwrap();
        let paid = reqwest::Client::new().get(url).header(mpp::AUTHORIZATION_HEADER, mpp::format_authorization(&credential).unwrap()).send().await.unwrap();
        let status = paid.status();
        let receipt = paid.headers().get(mpp::PAYMENT_RECEIPT_HEADER).map(|h| h.to_str().unwrap().to_string());
        println!("service answered {status}: {} (receipt {receipt:?})", paid.text().await.unwrap_or_default());
        assert!(status.is_success());
    }

    /// The cap is Tempo's, not only ours: a key authorized for $0.05 can't pay a $0.10 charge
    /// even when nothing in Sidefleet checks the price first. Needs the testnet, a funded
    /// wallet (`sidefleet wallet new && sidefleet wallet fund`) and its key file in
    /// `SIDEFLEET_TEST_WALLET_KEY`; run with `--ignored`.
    #[tokio::test]
    #[ignore]
    async fn tempo_refuses_a_payment_past_the_key_limit() {
        let root = crate::wallet::load(std::path::Path::new(&std::env::var("SIDEFLEET_TEST_WALLET_KEY").unwrap())).unwrap();
        let key = PrivateKeySigner::random();
        let mut o = offer(&key);
        o.expires_at = Utc::now() + chrono::Duration::hours(1);
        let auth = sign_authorization(&root, &o, 50_000).unwrap();
        let provider = TempoProvider::new(key, TESTNET_RPC)
            .unwrap()
            .with_signing_mode(TempoSigningMode::Keychain { wallet: root.address(), key_authorization: Some(Box::new(auth)), version: KeychainVersion::V2 })
            .with_expected_chain_id(TESTNET_CHAIN_ID);
        let url = "https://mpp.dev/api/ping/paid";
        let first = reqwest::get(url).await.unwrap();
        assert_eq!(first.status(), reqwest::StatusCode::PAYMENT_REQUIRED);
        let challenge = mpp::parse_www_authenticate(first.headers()[mpp::WWW_AUTHENTICATE_HEADER].to_str().unwrap()).unwrap();
        let charge: ChargeRequest = challenge.request.decode().unwrap();
        assert_eq!(charge.amount, "100000", "the test expects a $0.10 price");
        let paid = match provider.pay(&challenge).await {
            Err(e) => {
                println!("refused before sending: {e}");
                return;
            }
            Ok(credential) => {
                let header = mpp::format_authorization(&credential).unwrap();
                reqwest::Client::new().get(url).header(mpp::AUTHORIZATION_HEADER, header).send().await.unwrap()
            }
        };
        let status = paid.status();
        println!("service answered {status}: {}", paid.text().await.unwrap_or_default());
        assert!(!status.is_success(), "a key limited to $0.05 paid $0.10");
    }
}
