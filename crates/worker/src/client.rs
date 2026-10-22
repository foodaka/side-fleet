//! HTTP client for the control plane. Only outbound calls; the token travels
//! as a bearer header and is never logged.

use std::time::Duration;

use sidefleet_core::{
    Capabilities, ClaimRequest, FinishRunRequest, HeartbeatRequest, HeartbeatResponse, NewEvent,
    PostEventsRequest, RegisterWorkerRequest, Run, RunControl, RunStatus, Slots, UsageLimit, Worker,
};
use anyhow::{anyhow, Context, Result};
use reqwest::StatusCode;
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone)]
pub struct ControlClient {
    http: reqwest::Client,
    base: String,
    token: String,
    pub worker_id: String,
}

#[derive(Debug)]
pub enum ClientError {
    /// The control plane does not know this worker (restarted with a new DB?): re-register.
    NotRegistered,
    Other(anyhow::Error),
}

impl From<anyhow::Error> for ClientError {
    fn from(e: anyhow::Error) -> Self {
        ClientError::Other(e)
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NotRegistered => write!(f, "worker not registered with the control plane"),
            ClientError::Other(e) => write!(f, "{e:#}"),
        }
    }
}

impl ControlClient {
    pub fn new(base: &str, token: &str, worker_id: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            worker_id: worker_id.to_string(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    async fn check(r: reqwest::Response) -> Result<reqwest::Response, ClientError> {
        let status = r.status();
        if status == StatusCode::NOT_FOUND {
            // Distinguish "worker not registered" from a missing run by body.
            let body = r.text().await.unwrap_or_default();
            if body.contains("worker not registered") {
                return Err(ClientError::NotRegistered);
            }
            return Err(anyhow!("control plane returned 404: {body}").into());
        }
        if status == StatusCode::UNAUTHORIZED {
            return Err(anyhow!("control plane rejected the worker token (401)").into());
        }
        if !status.is_success() {
            let body = r.text().await.unwrap_or_default();
            return Err(anyhow!("control plane returned {status}: {body}").into());
        }
        Ok(r)
    }

    pub async fn register(&self, display_name: &str, caps: &Capabilities, active_runs: &[Uuid]) -> Result<Worker> {
        let r = self
            .http
            .post(self.url("/v1/worker/register"))
            .bearer_auth(&self.token)
            .json(&RegisterWorkerRequest {
                id: self.worker_id.clone(),
                display_name: display_name.to_string(),
                capabilities: caps.clone(),
                active_runs: active_runs.to_vec(),
            })
            .send()
            .await
            .context("register request failed")?;
        let r = Self::check(r).await.map_err(|e| anyhow!("{e}"))?;
        Ok(r.json().await?)
    }

    pub async fn heartbeat(&self, caps: &Capabilities, active_runs: &[Uuid]) -> Result<HeartbeatResponse, ClientError> {
        let r = self
            .http
            .post(self.url("/v1/worker/heartbeat"))
            .bearer_auth(&self.token)
            .json(&HeartbeatRequest { id: self.worker_id.clone(), capabilities: caps.clone(), active_runs: active_runs.to_vec() })
            .send()
            .await
            .context("heartbeat request failed")?;
        let r = Self::check(r).await?;
        if r.status() == StatusCode::NO_CONTENT {
            return Ok(HeartbeatResponse::default());
        }
        Ok(r.json().await.context("heartbeat response")?)
    }

    pub async fn update_ack(&self) -> Result<()> {
        let r = self
            .http
            .post(self.url("/v1/worker/update-ack"))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({"id": self.worker_id}))
            .send()
            .await
            .context("update-ack failed")?;
        Self::check(r).await.map_err(|e| anyhow!("{e}"))?;
        Ok(())
    }

    pub async fn claim(&self, repos: &[String], busy_repos: &[String], agents: &[String], slots: &Slots) -> Result<Option<Run>, ClientError> {
        let r = self
            .http
            .post(self.url("/v1/worker/claim"))
            .bearer_auth(&self.token)
            .json(&ClaimRequest {
                worker_id: self.worker_id.clone(),
                repos: repos.to_vec(),
                busy_repos: busy_repos.to_vec(),
                agents: agents.to_vec(),
                slots: Some(slots.clone()),
                kinds: vec!["agent".into(), "restore".into(), "fork".into()],
            })
            .send()
            .await
            .context("claim request failed")?;
        let r = Self::check(r).await?;
        if r.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        Ok(Some(r.json().await.context("claim response")?))
    }

    pub async fn post_events(&self, run_id: Uuid, events: Vec<NewEvent>) -> Result<RunControl> {
        let r = self
            .http
            .post(self.url(&format!("/v1/worker/runs/{run_id}/events?worker_id={}", self.worker_id)))
            .bearer_auth(&self.token)
            .json(&PostEventsRequest { events })
            .send()
            .await
            .context("post events failed")?;
        let r = Self::check(r).await.map_err(|e| anyhow!("{e}"))?;
        Ok(r.json().await?)
    }

    pub async fn control(&self, run_id: Uuid) -> Result<RunControl> {
        let r = self
            .http
            .get(self.url(&format!("/v1/worker/runs/{run_id}/control")))
            .bearer_auth(&self.token)
            .send()
            .await
            .context("control request failed")?;
        let r = Self::check(r).await.map_err(|e| anyhow!("{e}"))?;
        Ok(r.json().await?)
    }

    /// An image attached to a task, as stored on the control plane.
    pub async fn image(&self, id: Uuid) -> Result<Vec<u8>> {
        let r = self
            .http
            .get(self.url(&format!("/v1/worker/images/{id}")))
            .bearer_auth(&self.token)
            .send()
            .await
            .context("image request failed")?;
        let r = Self::check(r).await.map_err(|e| anyhow!("{e}"))?;
        Ok(r.bytes().await?.to_vec())
    }

    pub async fn finish(&self, run_id: Uuid, status: RunStatus, result: Option<Value>, usage_limit: Option<UsageLimit>) -> Result<Run> {
        let r = self
            .http
            .post(self.url(&format!("/v1/worker/runs/{run_id}/finish?worker_id={}", self.worker_id)))
            .bearer_auth(&self.token)
            .json(&FinishRunRequest { status, result, usage_limit })
            .send()
            .await
            .context("finish request failed")?;
        let r = Self::check(r).await.map_err(|e| anyhow!("{e}"))?;
        Ok(r.json().await?)
    }
}

/// A pending request to join a control plane without a token (`sidefleet connect`
/// with no `--token`): someone approves it on the Machines page.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Enrollment {
    pub id: Uuid,
    pub code: String,
    /// Proves to the control plane that we are the machine that asked. Never logged.
    pub secret: String,
    pub approve_url: String,
    #[serde(default = "default_poll")]
    pub poll_interval_secs: u64,
}

fn default_poll() -> u64 {
    2
}

#[derive(Debug)]
pub enum EnrollStatus {
    Pending,
    /// Approved: the worker token for this machine (handed out once) and the org's name.
    Approved { token: String, org: String },
    Denied,
    Expired,
}

/// Ask to join `base` as `worker_id`. Older control planes without enrollment return 404.
pub async fn enroll(base: &str, worker_id: &str, machine_name: &str, caps: &Capabilities) -> Result<Enrollment> {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?;
    let r = http
        .post(format!("{}/v1/enroll", base.trim_end_matches('/')))
        .json(&serde_json::json!({"worker_id": worker_id, "machine_name": machine_name, "capabilities": caps}))
        .send()
        .await
        .context("enroll request failed")?;
    match r.status() {
        s if s.is_success() => Ok(r.json().await.context("enroll response")?),
        StatusCode::NOT_FOUND => Err(anyhow!("this control plane does not support approving machines on the page; pass --token")),
        s => Err(anyhow!("control plane returned {s}: {}", r.text().await.unwrap_or_default())),
    }
}

/// One poll of a pending enrollment.
pub async fn enroll_status(base: &str, e: &Enrollment) -> Result<EnrollStatus> {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?;
    let r = http
        .get(format!("{}/v1/enroll/{}", base.trim_end_matches('/'), e.id))
        .bearer_auth(&e.secret)
        .send()
        .await
        .context("enrollment poll failed")?;
    let status = r.status();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    Ok(match status {
        StatusCode::OK => EnrollStatus::Approved {
            token: v["token"].as_str().ok_or_else(|| anyhow!("approved, but no token in the response"))?.to_string(),
            org: v["org"]["name"].as_str().unwrap_or_default().to_string(),
        },
        StatusCode::ACCEPTED => EnrollStatus::Pending,
        StatusCode::FORBIDDEN => EnrollStatus::Denied,
        StatusCode::GONE => EnrollStatus::Expired,
        s => return Err(anyhow!("control plane returned {s}: {}", v["error"].as_str().unwrap_or_default())),
    })
}

/// Exponential backoff helper for control plane errors: 1s, 2s, 4s ... capped.
pub struct Backoff {
    current: Duration,
    max: Duration,
}

impl Backoff {
    pub fn new(max: Duration) -> Self {
        Self { current: Duration::from_secs(1), max }
    }
    pub fn reset(&mut self) {
        self.current = Duration::from_secs(1);
    }
    pub fn next_delay(&mut self) -> Duration {
        let d = self.current;
        self.current = (self.current * 2).min(self.max);
        d
    }
}
