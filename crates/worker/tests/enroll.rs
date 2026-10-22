//! `sidefleet connect` without a token: the worker client asks to join, the
//! request is approved on the page, and the client collects a token that works.
//! Needs Postgres (docker compose up -d).

use sidefleet_core::Capabilities;
use sidefleet_server::test_support::{TestEnv, DEV_KEY};
use sidefleet_worker::client::{enroll, enroll_status, ControlClient, EnrollStatus};

#[tokio::test]
async fn connect_without_a_token_waits_for_approval_then_registers() {
    let env = TestEnv::start().await;
    let caps = Capabilities { hostname: "garage-mini".into(), ..Default::default() };
    let e = enroll(&env.base_url, "garage-mini", "Garage mini", &caps).await.unwrap();
    assert!(e.approve_url.contains(&e.code));
    assert!(matches!(enroll_status(&env.base_url, &e).await.unwrap(), EnrollStatus::Pending));

    let http = reqwest::Client::new();
    let r = http
        .post(format!("{}/v1/enrollments/{}/approve", env.base_url, e.id))
        .bearer_auth(DEV_KEY)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());

    let EnrollStatus::Approved { token, org } = enroll_status(&env.base_url, &e).await.unwrap() else { panic!("expected approval") };
    assert_eq!(org, "Default");
    assert!(matches!(enroll_status(&env.base_url, &e).await.unwrap(), EnrollStatus::Expired), "the token is handed out once");

    let client = ControlClient::new(&env.base_url, &token, "garage-mini").unwrap();
    let w = client.register("Garage mini", &caps, &[]).await.unwrap();
    assert_eq!(w.id, "garage-mini");
    client.heartbeat(&caps, &[]).await.unwrap();

    // Denied requests are reported as such.
    let d = enroll(&env.base_url, "intruder", "intruder", &Capabilities::default()).await.unwrap();
    http.post(format!("{}/v1/enrollments/{}/deny", env.base_url, d.id)).bearer_auth(DEV_KEY).json(&serde_json::json!({})).send().await.unwrap();
    assert!(matches!(enroll_status(&env.base_url, &d).await.unwrap(), EnrollStatus::Denied));

    env.teardown().await;
}
