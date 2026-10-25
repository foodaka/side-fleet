//! Sidefleet control plane: Axum + Postgres. Exposed as a library so tests
//! (and the worker's end-to-end tests) can spin up the exact router in-process.

pub mod approvals;
pub mod attention;
pub mod auth;
pub mod db;
pub mod events;
pub mod fork;
pub mod inbox;
pub mod mcp;
pub mod notify;
pub mod oauth;
pub mod push;
pub mod routes;
pub mod summary;
pub mod tenancy;
pub mod test_support;

use std::sync::Arc;

use sqlx::PgPool;

pub use routes::router;

/// Runtime configuration, normally read from the environment by `main`.
///
/// Single-tenant (self-hosting): `dev_key` and `worker_tokens` are set and
/// everything happens in the default org. Hosted: `github` is set, people sign
/// in and work in their own orgs; the key and static tokens are optional and,
/// when present, still act in the default org.
#[derive(Clone)]
pub struct Config {
    /// Bearer key for the developer API and the web page (empty: disabled).
    pub dev_key: String,
    /// Accepted static worker tokens (one per machine, or one shared); default org.
    pub worker_tokens: Vec<String>,
    /// Build of this control plane, shown on the page and sent to workers.
    pub build: String,
    /// Worker id of the machine hosting this control plane, when `serve` runs a
    /// built-in worker. The page puts it at the centre of the fleet map.
    pub host_worker: Option<String>,
    /// GitHub sign-in; turns on multi-tenant mode.
    pub github: Option<GithubOAuth>,
    /// Public address of this control plane (`https://app.sidefleet.xyz`), used for
    /// OAuth redirects, connect commands and secure cookies. Taken from the
    /// request's Host header when unset.
    pub public_url: Option<String>,
    /// Telegram chat told about every new sign-up (hosted mode). Optional.
    pub telegram: Option<Telegram>,
    /// Web push identity (`SIDEFLEET_VAPID_PRIVATE_KEY`); push is off without it.
    pub vapid: Option<push::Vapid>,
}

/// A GitHub OAuth app. The URLs are configurable so tests can stand in for GitHub.
#[derive(Clone)]
pub struct GithubOAuth {
    pub client_id: String,
    pub client_secret: String,
    pub authorize_url: String,
    pub token_url: String,
    pub api_url: String,
}

impl GithubOAuth {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client_id,
            client_secret,
            authorize_url: "https://github.com/login/oauth/authorize".into(),
            token_url: "https://github.com/login/oauth/access_token".into(),
            api_url: "https://api.github.com".into(),
        }
    }
}

/// A Telegram bot and the chat it posts to. The API URL is configurable so tests
/// can stand in for Telegram.
#[derive(Clone)]
pub struct Telegram {
    pub bot_token: String,
    pub chat_id: String,
    pub api_url: String,
}

impl Telegram {
    pub fn new(bot_token: String, chat_id: String) -> Self {
        Self { bot_token, chat_id, api_url: "https://api.telegram.org".into() }
    }
}

fn env_nonempty(k: &str) -> Option<String> {
    std::env::var(k).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let github = match (
            env_nonempty("SIDEFLEET_GITHUB_CLIENT_ID").or_else(|| env_nonempty("GITHUB_CLIENT_ID")),
            env_nonempty("SIDEFLEET_GITHUB_CLIENT_SECRET").or_else(|| env_nonempty("GITHUB_CLIENT_SECRET")),
        ) {
            (Some(id), Some(secret)) => Some(GithubOAuth::new(id, secret)),
            (None, None) => None,
            _ => anyhow::bail!("set both SIDEFLEET_GITHUB_CLIENT_ID and SIDEFLEET_GITHUB_CLIENT_SECRET for GitHub sign-in"),
        };
        let telegram = match (env_nonempty("TELEGRAM_BOT_TOKEN"), env_nonempty("TELEGRAM_CHAT_ID")) {
            (Some(token), Some(chat)) => Some(Telegram::new(token, chat)),
            (None, None) => None,
            _ => anyhow::bail!("set both TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID for sign-up messages"),
        };
        let dev_key = env_nonempty("SIDEFLEET_DEV_KEY").unwrap_or_default();
        let tokens = env_nonempty("SIDEFLEET_WORKER_TOKENS").or_else(|| env_nonempty("SIDEFLEET_WORKER_TOKEN")).unwrap_or_default();
        let worker_tokens: Vec<String> = tokens
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if github.is_none() {
            // Single-tenant: the key and at least one token are how anyone gets in.
            anyhow::ensure!(!dev_key.is_empty(), "SIDEFLEET_DEV_KEY is not set");
            anyhow::ensure!(!worker_tokens.is_empty(), "SIDEFLEET_WORKER_TOKENS is not set");
        }
        anyhow::ensure!(dev_key.is_empty() || dev_key.len() >= 8, "SIDEFLEET_DEV_KEY must be at least 8 characters");
        let build = std::env::var("SIDEFLEET_BUILD").unwrap_or_else(|_| "dev".into());
        let host_worker = env_nonempty("SIDEFLEET_HOST_WORKER");
        let public_url = env_nonempty("SIDEFLEET_PUBLIC_URL").map(|u| u.trim_end_matches('/').to_string());
        let vapid = match env_nonempty("SIDEFLEET_VAPID_PRIVATE_KEY") {
            Some(k) => {
                let subject = env_nonempty("SIDEFLEET_VAPID_SUBJECT")
                    .or_else(|| public_url.clone().filter(|u| u.starts_with("https://")))
                    .unwrap_or_else(|| push::DEFAULT_SUBJECT.into());
                Some(push::Vapid::from_private_key(&k, &subject)?)
            }
            None => None,
        };
        Ok(Self { dev_key, worker_tokens, build, host_worker, github, public_url, telegram, vapid })
    }

    /// Hosted (multi-tenant) mode: people sign in and have their own orgs.
    pub fn hosted(&self) -> bool {
        self.github.is_some()
    }

    /// Cookies get `Secure` when the public address is HTTPS.
    pub fn secure_cookies(&self) -> bool {
        self.public_url.as_deref().is_some_and(|u| u.starts_with("https://"))
    }
}

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<Config>,
    pub events: events::EventBus,
    pub inbox: inbox::Inbox,
}

impl AppState {
    /// Also starts the inbox's watcher when called inside a Tokio runtime.
    pub fn new(pool: PgPool, config: Config) -> Self {
        let state = Self { pool, config: Arc::new(config), events: events::EventBus::default(), inbox: inbox::Inbox::default() };
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(inbox::watch(state.clone()));
        }
        state
    }
}

/// Connect to Postgres and apply migrations.
pub async fn connect_and_migrate(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

/// Marks runs whose worker stopped heart-beating as failed, so the UI does not
/// show them as running forever. Runs every 15s.
pub async fn reaper(state: AppState) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        match db::reap_orphaned_runs(&state.pool, 90).await {
            Ok(reaped) => {
                for ev in reaped {
                    state.events.publish(ev);
                }
            }
            Err(e) => tracing::warn!(error = %e, "reaper failed"),
        }
    }
}
