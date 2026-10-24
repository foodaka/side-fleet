//! Helpers for integration tests (used by the server and worker tests): a fresh
//! Postgres database per test and an in-process server on a random port, plus a
//! stand-in for GitHub's OAuth endpoints for the hosted mode.
//! Requires Postgres; see README ("docker compose up -d").

use crate::{AppState, Config, GithubOAuth};
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
    /// Single-tenant: developer key and one static worker token.
    pub async fn start() -> TestEnv {
        Self::start_with(|_| {}).await
    }

    /// Hosted: GitHub sign-in (against [`FakeGithub`]) as well as the developer
    /// key and static token, which act in the default org.
    pub async fn start_hosted() -> TestEnv {
        let gh = FakeGithub::start().await;
        Self::start_with(|c| c.github = Some(gh)).await
    }

    /// Create a database named `sidefleet_test_<random>`, migrate it, and serve the
    /// router on 127.0.0.1:0 with the test config adjusted by `tweak`.
    pub async fn start_with(tweak: impl FnOnce(&mut Config)) -> TestEnv {
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
        let mut config = Config {
            dev_key: DEV_KEY.into(),
            worker_tokens: vec![WORKER_TOKEN.into()],
            build: "test".into(),
            host_worker: None,
            github: None,
            public_url: None,
            vapid: None,
        };
        tweak(&mut config);
        let state = AppState::new(pool.clone(), config);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = crate::router(state.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        TestEnv { pool, base_url: format!("http://{addr}"), db_name, state, server: Some(server) }
    }

    /// Sign in through the GitHub flow as `login` (any name works with
    /// [`FakeGithub`]) and return the session cookie value.
    pub async fn github_login(&self, login: &str) -> String {
        let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let r = http.get(format!("{}/auth/github/login", self.base_url)).send().await.unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::SEE_OTHER, "login should redirect to GitHub");
        let state_cookie = set_cookie_value(&r, "sf_oauth_state").expect("state cookie");
        let location = r.headers()["location"].to_str().unwrap().to_string();
        let url = reqwest::Url::parse(&location).unwrap();
        let state = url.query_pairs().find(|(k, _)| k == "state").unwrap().1.to_string();
        let r = http
            .get(format!("{}/auth/github/callback", self.base_url))
            .query(&[("code", login), ("state", state.as_str())])
            .header("cookie", format!("sf_oauth_state={state_cookie}"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::SEE_OTHER, "callback should redirect home: {}", r.text().await.unwrap_or_default());
        set_cookie_value(&r, "sf_session").expect("session cookie")
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

fn set_cookie_value(r: &reqwest::Response, name: &str) -> Option<String> {
    r.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| v.split(';').next()?.split_once('='))
        .find(|(k, v)| *k == name && !v.is_empty())
        .map(|(_, v)| v.to_string())
}

/// GitHub's OAuth and user endpoints, faked: the authorization code is taken as
/// the login, and every login is a user with a stable id.
pub struct FakeGithub;

impl FakeGithub {
    pub async fn start() -> GithubOAuth {
        use axum::{extract::Json, http::HeaderMap, routing::{get, post}, Router};
        use serde_json::{json, Value};

        fn login_from(h: &HeaderMap) -> Option<String> {
            h.get("authorization")?.to_str().ok()?.strip_prefix("Bearer gho_").map(str::to_owned)
        }
        fn id_of(login: &str) -> i64 {
            // FNV-1a, positive: the same login is always the same GitHub user.
            let mut h: u64 = 0xcbf29ce484222325;
            for b in login.bytes() {
                h = (h ^ b as u64).wrapping_mul(0x100000001b3);
            }
            (h >> 1) as i64
        }
        let app = Router::new()
            .route("/login/oauth/access_token", post(|Json(v): Json<Value>| async move {
                let code = v["code"].as_str().unwrap_or_default();
                Json(json!({ "access_token": format!("gho_{code}"), "token_type": "bearer" }))
            }))
            .route("/user", get(|h: HeaderMap| async move {
                match login_from(&h) {
                    Some(l) => Json(json!({ "id": id_of(&l), "login": l, "email": null })).into_response(),
                    None => axum::http::StatusCode::UNAUTHORIZED.into_response(),
                }
            }))
            .route("/user/emails", get(|h: HeaderMap| async move {
                let l = login_from(&h).unwrap_or_default();
                Json(json!([{ "email": format!("{l}@example.com"), "primary": true, "verified": true }]))
            }));
        use axum::response::IntoResponse;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        GithubOAuth {
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
            authorize_url: format!("{base}/login/oauth/authorize"),
            token_url: format!("{base}/login/oauth/access_token"),
            api_url: base,
        }
    }
}
