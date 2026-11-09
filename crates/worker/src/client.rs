//! HTTP client for the control plane. Only outbound calls; the token travels
//! as a bearer header and is never logged.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sidefleet_core::{
    Approval, ApprovalRequest, Capabilities, ClaimRequest, FinishRunRequest, HeartbeatRequest, HeartbeatResponse, LiveTextRequest, WorktreeCleanupResult,
    NewEvent, PostEventsRequest, PrReport, PrReports, RegisterWorkerRequest, Run, RunControl, RunStatus, Slots, UsageLimit, Worker,
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
    /// The control plane said it takes gzip bodies; shared by every clone.
    gzip: Arc<AtomicBool>,
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

/// The control plane, or something in front of it such as a firewall, refused the
/// request itself: sending the same request again gets the same answer.
#[derive(Debug)]
pub struct Rejected {
    pub status: StatusCode,
    /// What came back, in brief (see [`brief_body`]).
    pub detail: String,
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "control plane returned {}: {}", self.status, self.detail)
    }
}

impl std::error::Error for Rejected {}

/// An error page in a line: an HTML page's title and request id (a firewall's
/// "Blocked" page is a couple of hundred kilobytes of inline fonts), or the
/// start of anything else.
pub fn brief_body(body: &str) -> String {
    const MAX: usize = 300;
    if !body.trim_start().starts_with('<') {
        let body = body.trim();
        return match body.char_indices().nth(MAX) {
            Some((i, _)) => format!("{}…", &body[..i]),
            None => body.to_string(),
        };
    }
    let between = |open: &str, close: &str| {
        let start = body.find(open)? + open.len();
        Some(body[start..].split(close).next()?.trim().to_string())
    };
    let title = between("<title>", "</title>").filter(|t| !t.is_empty()).unwrap_or_else(|| "an HTML page".into());
    match between("Request ID:", "<").filter(|id| !id.is_empty()) {
        Some(id) => format!("{title} (request id {id})"),
        None => title,
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
            gzip: Arc::new(AtomicBool::new(false)),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// Whether to gzip what carries the agent's output, as the control plane said
    /// on its last heartbeat (an older one takes none).
    pub fn set_gzip(&self, on: bool) {
        self.gzip.store(on, Ordering::Relaxed);
    }

    /// `body` as JSON, gzipped when the control plane takes it: smaller, and a firewall
    /// in front of it can't mistake the agent's output (paths, code, logs) for an attack.
    fn body(&self, req: reqwest::RequestBuilder, body: &impl serde::Serialize) -> Result<reqwest::RequestBuilder> {
        let json = serde_json::to_vec(body)?;
        let req = req.header(reqwest::header::CONTENT_TYPE, "application/json");
        if !self.gzip.load(Ordering::Relaxed) {
            return Ok(req.body(json));
        }
        let mut enc = flate2::write::GzEncoder::new(Vec::with_capacity(json.len() / 4), flate2::Compression::fast());
        enc.write_all(&json)?;
        Ok(req.header(reqwest::header::CONTENT_ENCODING, "gzip").body(enc.finish()?))
    }

    async fn check(r: reqwest::Response) -> Result<reqwest::Response, ClientError> {
        let status = r.status();
        if status == StatusCode::NOT_FOUND {
            // Distinguish "worker not registered" from a missing run by body.
            let body = r.text().await.unwrap_or_default();
            if body.contains("worker not registered") {
                return Err(ClientError::NotRegistered);
            }
            return Err(anyhow::Error::new(Rejected { status, detail: brief_body(&body) }).into());
        }
        if status == StatusCode::UNAUTHORIZED {
            return Err(anyhow!("control plane rejected the worker token (401)").into());
        }
        if !status.is_success() {
            let detail = brief_body(&r.text().await.unwrap_or_default());
            // Timeouts and rate limits pass; any other 4xx is about the request itself.
            if status.is_client_error() && status != StatusCode::REQUEST_TIMEOUT && status != StatusCode::TOO_MANY_REQUESTS {
                return Err(anyhow::Error::new(Rejected { status, detail }).into());
            }
            return Err(anyhow!("control plane returned {status}: {detail}").into());
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

    /// What this machine found out about threads' pull requests.
    pub async fn report_prs(&self, prs: Vec<PrReport>) -> Result<()> {
        let r = self
            .http
            .post(self.url("/v1/worker/prs"))
            .bearer_auth(&self.token)
            .json(&PrReports { worker_id: self.worker_id.clone(), prs })
            .send()
            .await
            .context("pull request report failed")?;
        Self::check(r).await.map_err(|e| anyhow!("{e}"))?;
        Ok(())
    }

    pub async fn worktree_cleanup_done(&self, result: &WorktreeCleanupResult) -> Result<()> {
        let r = self
            .http
            .post(self.url("/v1/worker/worktree-cleanup"))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({"id": self.worker_id, "result": result}))
            .send()
            .await
            .context("worktree-cleanup failed")?;
        Self::check(r).await.map_err(|e| anyhow!("{e}"))?;
        Ok(())
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

    /// Ask for a run, letting the control plane hold the request up to `wait` until
    /// one is claimable (older control planes answer at once).
    pub async fn claim(
        &self,
        repos: &[String],
        busy_repos: &[String],
        agents: &[String],
        slots: &Slots,
        wait: Duration,
    ) -> Result<Option<Run>, ClientError> {
        let r = self
            .http
            .post(self.url("/v1/worker/claim"))
            .bearer_auth(&self.token)
            // Outlast the wait, or a slow answer would be cut off as a timeout.
            .timeout(wait + Duration::from_secs(30))
            .json(&ClaimRequest {
                worker_id: self.worker_id.clone(),
                repos: repos.to_vec(),
                busy_repos: busy_repos.to_vec(),
                agents: agents.to_vec(),
                slots: Some(slots.clone()),
                kinds: vec!["agent".into(), "restore".into(), "fork".into(), "pr".into(), "integrate".into()],
                wait_secs: (!wait.is_zero()).then_some(wait.as_secs()),
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

    /// A refused post fails with [`Rejected`] (see `downcast_ref`).
    pub async fn post_events(&self, run_id: Uuid, events: Vec<NewEvent>) -> Result<RunControl> {
        let req = self.http.post(self.url(&format!("/v1/worker/runs/{run_id}/events?worker_id={}", self.worker_id))).bearer_auth(&self.token);
        let r = self
            .body(req, &PostEventsRequest { events })?
            .send()
            .await
            .context("post events failed")?;
        let r = Self::check(r).await.map_err(|e| match e {
            ClientError::Other(e) => e,
            e => anyhow!("{e}"),
        })?;
        Ok(r.json().await?)
    }

    /// Show the text the agent is writing to whoever follows the run. Best effort:
    /// Ok(false) means the control plane takes no live text (it predates it).
    pub async fn post_live(&self, run_id: Uuid, text: &str) -> Result<bool> {
        let req = self
            .http
            .post(self.url(&format!("/v1/worker/runs/{run_id}/live?worker_id={}", self.worker_id)))
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(5));
        let r = self
            .body(req, &LiveTextRequest { text: text.to_string() })?
            .send()
            .await
            .context("post live text failed")?;
        if matches!(r.status(), StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED) {
            return Ok(false);
        }
        Self::check(r).await.map_err(|e| anyhow!("{e}"))?;
        Ok(true)
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

    /// Ask the person on the page about a tool call ("Ask me").
    pub async fn request_approval(&self, run_id: Uuid, req: &ApprovalRequest) -> Result<Approval> {
        let post = self.http.post(self.url(&format!("/v1/worker/runs/{run_id}/approvals?worker_id={}", self.worker_id))).bearer_auth(&self.token);
        let r = self
            .body(post, req)?
            .send()
            .await
            .context("approval request failed")?;
        let r = Self::check(r).await.map_err(|e| anyhow!("{e}"))?;
        Ok(r.json().await?)
    }

    /// An approval as it stands (expired or cancelled once past its deadline or run).
    pub async fn approval(&self, run_id: Uuid, id: Uuid) -> Result<Approval> {
        let r = self
            .http
            .get(self.url(&format!("/v1/worker/runs/{run_id}/approvals/{id}")))
            .bearer_auth(&self.token)
            .send()
            .await
            .context("approval poll failed")?;
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
        let req = self.http.post(self.url(&format!("/v1/worker/runs/{run_id}/finish?worker_id={}", self.worker_id))).bearer_auth(&self.token);
        let r = self
            .body(req, &FinishRunRequest { status, result, usage_limit })?
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

#[cfg(test)]
mod tests {
    use super::brief_body;

    #[test]
    fn error_pages_are_summed_up_in_a_line() {
        let fonts = "x".repeat(200_000);
        let page = format!(
            "<!DOCTYPE html><html><head><title>Blocked</title><style>src: url(data:font/woff2;base64,{fonts})</style></head>\
             <body><p>Request ID: a549e02b6d1418ee</p><p>Your IP address: 192.0.2.1</p></body></html>"
        );
        assert_eq!(brief_body(&page), "Blocked (request id a549e02b6d1418ee)");
        assert_eq!(brief_body("<html><body>nope</body></html>"), "an HTML page");
        assert_eq!(brief_body("  run not found\n"), "run not found");
        let long = "é".repeat(400);
        assert_eq!(brief_body(&long).chars().count(), 301, "300 characters and an ellipsis, cut on a char boundary");
    }
}
