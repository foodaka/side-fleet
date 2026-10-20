//! Helpers for integration tests (used by the server and worker tests): a fresh
//! Postgres database per test and an in-process server on a random port.
//! Requires Postgres; see README ("docker compose up -d").

use crate::{AppState, Config};
use sqlx::{Connection, PgConnection, PgPool};

pub const DEV_KEY: &str = "test-dev-key-0000";
pub const WORKER_TOKEN: &str = "test-worker-token-0000";

pub fn base_database_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| "postgres://sidefleet:sidefleet@localhost:5432/sidefleet".into())
}

pub struct TestEnv {
    pub pool: PgPool,
    pub base_url: String,
    pub db_name: String,
    pub state: AppState,
    server: Option<tokio::task::JoinHandle<()>>,
}

impl TestEnv {
    /// Create a database named `sidefleet_test_<random>`, migrate it, and serve the
    /// router on 127.0.0.1:0.
    pub async fn start() -> TestEnv {
        let base = base_database_url();
        let db_name = format!("sidefleet_test_{}", uuid::Uuid::new_v4().simple());
        let mut conn = PgConnection::connect(&base).await.unwrap_or_else(|e| {
            panic!("cannot connect to Postgres at {base}: {e}. Start it with `docker compose up -d`.")
        });
        sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
            .execute(&mut conn)
            .await
            .expect("create test database");
        drop(conn);

        let url = replace_db_name(&base, &db_name);
        let pool = crate::connect_and_migrate(&url).await.expect("migrate test database");
        let config = Config { dev_key: DEV_KEY.into(), worker_tokens: vec![WORKER_TOKEN.into()], build: "test".into() };
        let state = AppState::new(pool.clone(), config);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = crate::router(state.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        TestEnv { pool, base_url: format!("http://{addr}"), db_name, state, server: Some(server) }
    }

    /// Stop the server and drop the test database.
    pub async fn teardown(mut self) {
        if let Some(s) = self.server.take() {
            s.abort();
        }
        self.pool.close().await;
        let base = base_database_url();
        if let Ok(mut conn) = PgConnection::connect(&base).await {
            let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)", self.db_name))
                .execute(&mut conn)
                .await;
        }
    }
}

fn replace_db_name(url: &str, db: &str) -> String {
    // postgres://user:pass@host:port/dbname[?params]
    let (head, tail) = match url.rfind('/') {
        Some(i) => (&url[..i], &url[i + 1..]),
        None => return url.to_string(),
    };
    let params = tail.find('?').map(|i| &tail[i..]).unwrap_or("");
    format!("{head}/{db}{params}")
}
