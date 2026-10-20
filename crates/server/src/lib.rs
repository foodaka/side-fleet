//! Sidefleet control plane: Axum + Postgres. Exposed as a library so tests
//! (and the worker's end-to-end tests) can spin up the exact router in-process.

pub mod auth;
pub mod db;
pub mod events;
pub mod routes;
pub mod test_support;

use std::sync::Arc;

use sqlx::PgPool;

pub use routes::router;

/// Runtime configuration, normally read from the environment by `main`.
#[derive(Clone)]
pub struct Config {
    /// Bearer key for the developer API and the web page.
    pub dev_key: String,
    /// Accepted worker tokens (one per machine, or one shared).
    pub worker_tokens: Vec<String>,
    /// Build of this control plane, shown on the page and sent to workers.
    pub build: String,
    /// Worker id of the machine hosting this control plane, when `serve` runs a
    /// built-in worker. The page puts it at the centre of the fleet map.
    pub host_worker: Option<String>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let dev_key = std::env::var("SIDEFLEET_DEV_KEY")
            .map_err(|_| anyhow::anyhow!("SIDEFLEET_DEV_KEY is not set"))?;
        let tokens = std::env::var("SIDEFLEET_WORKER_TOKENS")
            .or_else(|_| std::env::var("SIDEFLEET_WORKER_TOKEN"))
            .map_err(|_| anyhow::anyhow!("SIDEFLEET_WORKER_TOKENS is not set"))?;
        let worker_tokens: Vec<String> = tokens
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        anyhow::ensure!(!worker_tokens.is_empty(), "SIDEFLEET_WORKER_TOKENS is empty");
        anyhow::ensure!(dev_key.len() >= 8, "SIDEFLEET_DEV_KEY must be at least 8 characters");
        let build = std::env::var("SIDEFLEET_BUILD").unwrap_or_else(|_| "dev".into());
        let host_worker = std::env::var("SIDEFLEET_HOST_WORKER").ok().filter(|s| !s.is_empty());
        Ok(Self { dev_key, worker_tokens, build, host_worker })
    }
}

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<Config>,
    pub events: events::EventBus,
}

impl AppState {
    pub fn new(pool: PgPool, config: Config) -> Self {
        Self { pool, config: Arc::new(config), events: events::EventBus::default() }
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
