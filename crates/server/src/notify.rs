//! Messages to whoever runs a hosted control plane: a Telegram message for every
//! new sign-up. Optional (`TELEGRAM_BOT_TOKEN` and `TELEGRAM_CHAT_ID`). Sending
//! happens in the background and never holds up or fails a sign-in.

use crate::{AppState, Telegram};

/// `login` signed in for the first time.
pub fn new_user(state: &AppState, login: &str, email: Option<&str>) {
    let Some(tg) = state.config.telegram.clone() else { return };
    let pool = state.pool.clone();
    let (login, email) = (login.to_string(), email.map(str::to_string));
    tokio::spawn(async move {
        let users: Option<i64> = sqlx::query_scalar("SELECT count(*) FROM users").fetch_one(&pool).await.ok();
        let text = [
            Some("Sidefleet: new sign-up".to_string()),
            Some(format!("GitHub: {login} (https://github.com/{login})")),
            email.map(|e| format!("Email: {e}")),
            users.map(|n| format!("Users: {n}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
        send(&tg, &text).await;
    });
}

async fn send(tg: &Telegram, text: &str) {
    let Ok(http) = reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build() else { return };
    let url = format!("{}/bot{}/sendMessage", tg.api_url.trim_end_matches('/'), tg.bot_token);
    let body = serde_json::json!({ "chat_id": tg.chat_id, "text": text, "disable_web_page_preview": true });
    match http.post(url).json(&body).send().await {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => tracing::warn!(status = %r.status(), "telegram message was refused"),
        // The URL carries the bot token: keep it out of the log.
        Err(e) => tracing::warn!(error = %e.without_url(), "telegram message failed"),
    }
}
