//! `sidefleet wallet`: the user's root key, kept on their own laptop, never on a
//! machine that runs tasks. It signs each task's access key for that task's
//! budget (`authorize`); the control plane only passes the signed authorization
//! on to the worker. Testnet only, for now.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use mpp::PrivateKeySigner;
use serde_json::{json, Value};
use sidefleet_core::{AuthorizePayKeyRequest, PayKeyInfo};
use uuid::Uuid;

use crate::pay::{self, usd};

/// Make a new root key at `path` (owner-only). Refuses to replace one.
pub fn create(path: &Path) -> Result<String> {
    use std::os::unix::fs::OpenOptionsExt;
    if path.exists() {
        bail!("{} already exists; remove it first to make a new wallet", path.display());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let signer = PrivateKeySigner::random();
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    writeln!(f, "0x{}", alloy::hex::encode(signer.to_bytes()))?;
    Ok(signer.address().to_checksum(None))
}

pub fn load(path: &Path) -> Result<PrivateKeySigner> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {} (make one with `sidefleet wallet new`)", path.display()))?;
    text.trim().parse::<PrivateKeySigner>().context("the wallet file does not hold a private key")
}

async fn rpc(url: &str, method: &str, params: Value) -> Result<Value> {
    let resp: Value = reqwest::Client::new()
        .post(url)
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
        .send()
        .await
        .with_context(|| format!("calling {method}"))?
        .json()
        .await?;
    if let Some(e) = resp.get("error") {
        bail!("{method} failed: {e}");
    }
    Ok(resp.get("result").cloned().unwrap_or(Value::Null))
}

/// Ask the testnet faucet for test dollars.
pub async fn fund(rpc_url: &str, address: &str) -> Result<Value> {
    rpc(rpc_url, "tempo_fundAddress", json!([address])).await
}

/// The wallet's balance of `token`, in micros.
pub async fn balance(rpc_url: &str, address: &str, token: &str) -> Result<u128> {
    // balanceOf(address)
    let data = format!("0x70a08231{:0>64}", address.trim_start_matches("0x").to_ascii_lowercase());
    let out = rpc(rpc_url, "eth_call", json!([{"to": token, "data": data}, "latest"])).await?;
    let hex = out.as_str().unwrap_or("0x0").trim_start_matches("0x").trim_start_matches('0');
    Ok(if hex.is_empty() { 0 } else { u128::from_str_radix(hex, 16)? })
}

/// Where `authorize` talks to the control plane: its URL and a credential (an API
/// token with full access, or the developer key of a self-hosted control plane).
pub struct Control {
    pub url: String,
    pub token: String,
}

impl Control {
    async fn info(&self, http: &reqwest::Client, run: Uuid) -> Result<PayKeyInfo> {
        let r = http.get(format!("{}/v1/runs/{run}/pay-key", self.url)).bearer_auth(&self.token).send().await?;
        let status = r.status();
        if !status.is_success() {
            bail!("the control plane answered {status}: {}", r.text().await.unwrap_or_default());
        }
        Ok(r.json().await?)
    }
}

/// Sign `run`'s key for its budget with the root key, after showing what it allows.
/// Waits for the machine to make the key when the task has only just started.
pub async fn authorize(control: &Control, root: &PrivateKeySigner, run: Uuid, confirm: impl Fn(&str) -> bool) -> Result<()> {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?;
    let mut waited = Duration::ZERO;
    let (info, key) = loop {
        let info = control.info(&http, run).await?;
        if info.authorized {
            bail!("this task's budget is already authorized");
        }
        match info.key.clone() {
            Some(k) => break (info, k),
            None if info.budget_micros.is_none() => bail!("this task has no budget"),
            None if waited >= Duration::from_secs(300) => bail!("no machine has made a key for this task yet; is it running?"),
            None => {
                if waited.is_zero() {
                    eprintln!("Waiting for the machine to pick the task up and make its key...");
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                waited += Duration::from_secs(3);
            }
        }
    };
    let budget = info.budget_micros.context("this task has no budget")?;
    let summary = format!(
        "Task: {}\nAllow its key {} to spend up to {} (in {}) until {}.\nPayments over {} still ask you first.\nWallet: {}",
        info.task.lines().next().unwrap_or_default().chars().take(100).collect::<String>(),
        key.address,
        usd(budget),
        key.token,
        key.expires_at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M"),
        usd(info.ask_above_micros.unwrap_or(sidefleet_core::DEFAULT_ASK_ABOVE_MICROS)),
        root.address().to_checksum(None),
    );
    if !confirm(&summary) {
        bail!("not authorized");
    }
    let signed = pay::sign_authorization(root, &key, budget)?;
    let body = AuthorizePayKeyRequest { wallet: root.address().to_checksum(None), authorization: serde_json::to_value(&signed)?, key_id: None };
    let r = http.post(format!("{}/v1/runs/{run}/pay-key/authorize", control.url)).bearer_auth(&control.token).json(&body).send().await?;
    let status = r.status();
    if !status.is_success() {
        bail!("the control plane answered {status}: {}", r.text().await.unwrap_or_default());
    }
    Ok(())
}
