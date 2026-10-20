use acd::{connect_and_migrate, AppState, Config};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "acd=info,tower_http=warn".into()))
        .init();

    let config = Config::from_env()?;
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://acd:acd@localhost:5432/acd".into());
    let bind = std::env::var("ACD_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into());

    let pool = connect_and_migrate(&database_url).await?;
    let state = AppState::new(pool, config);
    tokio::spawn(acd::reaper(state.clone()));

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "acd listening; open http://{bind}/ in a browser");
    axum::serve(listener, acd::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
