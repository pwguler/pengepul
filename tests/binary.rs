use std::net::SocketAddr;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pengepul::tokens::save_token;
use pengepul::types::{ProviderId, TokenData};
use serde_json::json;
use tokio::sync::Notify;

#[test]
fn binary_help_runs_cli() {
    let output = Command::new(env!("CARGO_BIN_EXE_pengepul"))
        .arg("help")
        .output()
        .expect("run binary");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("Usage: pengepul"));
}

// ---------------------------------------------------------------------------
// cli-hardening: SIGTERM and SIGINT drain the relay for 15 s, then it exits
// ---------------------------------------------------------------------------

/// A configured endpoint whose chat completions wait for the test: `arrived` fires when a
/// request reaches it, and it answers once `release` does, or never.
struct HeldEndpoint {
    addr: SocketAddr,
    arrived: Arc<Notify>,
    release: Arc<Notify>,
}

async fn held_endpoint() -> HeldEndpoint {
    let arrived = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback port");
    let addr = listener.local_addr().expect("bound address");
    let (on_arrival, on_release) = (Arc::clone(&arrived), Arc::clone(&release));
    let endpoint = axum::Router::new()
        .route(
            "/v1/chat/completions",
            axum::routing::post(move || {
                let (on_arrival, on_release) = (Arc::clone(&on_arrival), Arc::clone(&on_release));
                async move {
                    on_arrival.notify_one();
                    on_release.notified().await;
                    axum::Json(json!({
                        "id": "chatcmpl-1",
                        "object": "chat.completion",
                        "model": "m",
                        "choices": [{
                            "index": 0,
                            "message": {"role": "assistant", "content": "pong"},
                            "finish_reason": "stop"
                        }],
                        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                    }))
                }
            }),
        )
        .route(
            "/v1/models",
            axum::routing::get(|| async { axum::Json(json!({"data": []})) }),
        );
    tokio::spawn(async move {
        let _ = axum::serve(listener, endpoint).await;
    });
    HeldEndpoint {
        addr,
        arrived,
        release,
    }
}

/// `pengepul serve` in front of an endpoint, as the configured provider `slow` with one key.
/// Killed on drop, so a failed test leaves no relay behind.
struct Relay {
    child: Child,
    addr: SocketAddr,
    _home: tempfile::TempDir,
}

impl Relay {
    async fn start(endpoint: SocketAddr) -> Self {
        let home = tempfile::tempdir().expect("tempdir");
        let auth_dir = home.path().join("auth");
        save_token(
            &auth_dir,
            &TokenData {
                access_token: "sk-slow".to_string(),
                refresh_token: String::new(),
                email: "key-1".to_string(),
                expires_at: String::new(),
                account_uuid: "acct-key-1".to_string(),
                provider: ProviderId::generic("slow"),
                id_token: None,
                last_refresh_at: None,
                plan_type: None,
            },
        )
        .expect("save key");
        let config = home.path().join("config.yaml");
        std::fs::write(
            &config,
            format!(
                "api-keys:\n  - sk-test\nauth-dir: {}\nproviders:\n  slow:\n    base-url: http://{endpoint}/v1\n",
                auth_dir.display()
            ),
        )
        .expect("write config");
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a free port")
            .port();
        let child = Command::new(env!("CARGO_BIN_EXE_pengepul"))
            .arg("serve")
            .arg("--config")
            .arg(&config)
            .arg("--port")
            .arg(port.to_string())
            .env("HOME", home.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn pengepul serve");
        let relay = Self {
            child,
            addr: SocketAddr::from(([127, 0, 0, 1], port)),
            _home: home,
        };
        let health = format!("http://{}/health", relay.addr);
        let deadline = Instant::now() + Duration::from_secs(10);
        while reqwest::get(&health).await.is_err() {
            assert!(Instant::now() < deadline, "the relay never came up");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        relay
    }

    /// Send a chat request through the relay, answered whenever the endpoint answers.
    fn ask(&self) -> tokio::task::JoinHandle<reqwest::Result<(u16, String)>> {
        let url = format!("http://{}/v1/chat/completions", self.addr);
        tokio::spawn(async move {
            let response = reqwest::Client::new()
                .post(url)
                .bearer_auth("sk-test")
                .json(&json!({
                    "model": "slow/m",
                    "messages": [{"role": "user", "content": "reply exactly: pong"}]
                }))
                .send()
                .await?;
            let status = response.status().as_u16();
            Ok((status, response.text().await?))
        })
    }

    fn signal(&self, name: &str) {
        let sent = Command::new("kill")
            .args(["-s", name, &self.child.id().to_string()])
            .status()
            .expect("run kill");
        assert!(sent.success(), "kill -s {name} failed");
    }

    /// Whether a new connection is refused, which is what a closed listener answers.
    async fn refuses_connections(&self) -> bool {
        tokio::net::TcpStream::connect(self.addr).await.is_err()
    }

    async fn exit_within(&mut self, limit: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().expect("poll the relay") {
                return Some(status);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        None
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A stop signal closes the door and lets the request already inside finish: the listener
/// goes at once, the reply still arrives whole, and only then does the relay exit.
async fn drains_on(signal: &str) {
    let endpoint = held_endpoint().await;
    let mut relay = Relay::start(endpoint.addr).await;
    let in_flight = relay.ask();
    tokio::time::timeout(Duration::from_secs(10), endpoint.arrived.notified())
        .await
        .expect("the request never reached the endpoint");

    relay.signal(signal);

    let deadline = Instant::now() + Duration::from_secs(5);
    while !relay.refuses_connections().await {
        assert!(
            Instant::now() < deadline,
            "SIG{signal}: still accepting connections"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    endpoint.release.notify_one();
    let (status, body) = in_flight
        .await
        .expect("the request task")
        .unwrap_or_else(|error| panic!("SIG{signal}: the request in flight was cut: {error}"));
    assert_eq!(status, 200, "SIG{signal}: {body}");
    assert!(body.contains(r#""content":"pong""#), "SIG{signal}: {body}");
    let exit = relay
        .exit_within(Duration::from_secs(5))
        .await
        .unwrap_or_else(|| panic!("SIG{signal}: the relay did not exit once drained"));
    assert!(exit.success(), "SIG{signal}: exited {exit}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sigterm_lets_a_request_in_flight_finish_then_exits() {
    drains_on("TERM").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sigint_lets_a_request_in_flight_finish_then_exits() {
    drains_on("INT").await;
}

/// The drain is a bound, not a wait for whatever is running: a request still running 15 s
/// after the signal is cut there, and the relay exits on the mark rather than with it.
/// Waits out the whole drain, so this one test costs 15 s.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_still_running_at_15_s_does_not_keep_the_relay_alive() {
    let endpoint = held_endpoint().await;
    let mut relay = Relay::start(endpoint.addr).await;
    let in_flight = relay.ask();
    tokio::time::timeout(Duration::from_secs(10), endpoint.arrived.notified())
        .await
        .expect("the request never reached the endpoint");

    let signalled = Instant::now();
    relay.signal("TERM");
    let exit = relay
        .exit_within(Duration::from_secs(25))
        .await
        .expect("the relay outlived its drain");
    let took = signalled.elapsed();

    assert!(
        (Duration::from_millis(14_500)..=Duration::from_secs(17)).contains(&took),
        "exited {took:?} after SIGTERM, not at the 15 s mark"
    );
    assert!(exit.success(), "exited {exit}");
    assert!(
        in_flight.await.expect("the request task").is_err(),
        "a request the endpoint never answered came back answered"
    );
}
