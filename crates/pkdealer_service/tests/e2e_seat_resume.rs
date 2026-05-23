//! End-to-end tests for seat resume via `client_secret`.
//!
//! Covers EPIC-20's last close-out item: a crashed agent process must be
//! able to re-attach to its seat by presenting the same `client_secret`
//! and receive the same `player_token` and `seat_number`.

use std::{
    io,
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command},
    time::{Duration, Instant},
};

use pkdealer_proto::dealer::{
    SeatPlayerAtRequest, SeatPlayerRequest, dealer_service_client::DealerServiceClient,
    seat_player_at_response, seat_player_response,
};
use tonic::Request;

// ── process helpers (mirrors e2e_two_players.rs) ─────────────────────────────

struct ChildProcessGuard {
    child: Child,
}

impl Drop for ChildProcessGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn reserve_local_port() -> io::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

fn service_bin_path() -> io::Result<PathBuf> {
    std::env::var("CARGO_BIN_EXE_pkdealer_service")
        .map(PathBuf::from)
        .map_err(|e| io::Error::new(io::ErrorKind::NotFound, e))
}

async fn wait_for_service_ready(endpoint: &str, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if let Ok(mut c) = DealerServiceClient::connect(endpoint.to_owned()).await
            && c.ping(Request::new(pkdealer_proto::new_ping_request("ready")))
                .await
                .is_ok()
        {
            return true;
        }
        if start.elapsed() > timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn spawn_service() -> (ChildProcessGuard, String) {
    let port = reserve_local_port().expect("port");
    let bin = service_bin_path().expect("service bin");
    let child = Command::new(&bin)
        .env("PKDEALER_PORT", port.to_string())
        .env("OTEL_SDK_DISABLED", "true")
        .spawn()
        .expect("spawn pkdealer_service");
    let guard = ChildProcessGuard { child };
    let endpoint = format!("http://127.0.0.1:{port}");
    assert!(
        wait_for_service_ready(&endpoint, Duration::from_secs(5)).await,
        "service did not become ready",
    );
    (guard, endpoint)
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_with_secret_returns_same_seat_and_token() {
    let (_guard, endpoint) = spawn_service().await;
    let mut client = DealerServiceClient::connect(endpoint)
        .await
        .expect("connect");

    // First call: fresh seat allocation.
    let first = client
        .seat_player(Request::new(SeatPlayerRequest {
            name: "alice".to_owned(),
            chips: 10_000,
            client_secret: "alice-secret-abc".to_owned(),
        }))
        .await
        .expect("first seat")
        .into_inner();
    let first_seat = match first.result {
        Some(seat_player_response::Result::SeatNumber(s)) => s,
        other => panic!("expected SeatNumber, got {other:?}"),
    };
    assert!(!first.player_token.is_empty(), "first token populated");
    assert!(!first.resumed, "first call is not a resume");

    // Second call with the same secret: same seat + same token, resumed=true.
    let second = client
        .seat_player(Request::new(SeatPlayerRequest {
            name: "alice".to_owned(),
            chips: 99_999, // should be ignored on resume
            client_secret: "alice-secret-abc".to_owned(),
        }))
        .await
        .expect("second seat")
        .into_inner();
    let second_seat = match second.result {
        Some(seat_player_response::Result::SeatNumber(s)) => s,
        other => panic!("expected SeatNumber, got {other:?}"),
    };

    assert_eq!(first_seat, second_seat, "same seat on resume");
    assert_eq!(
        first.player_token, second.player_token,
        "same token on resume",
    );
    assert!(second.resumed, "second call is a resume");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seat_without_secret_does_not_register_for_resume() {
    let (_guard, endpoint) = spawn_service().await;
    let mut client = DealerServiceClient::connect(endpoint)
        .await
        .expect("connect");

    // No secret on first call → no resume binding.
    let first = client
        .seat_player(Request::new(SeatPlayerRequest {
            name: "bob".to_owned(),
            chips: 10_000,
            client_secret: String::new(),
        }))
        .await
        .expect("first seat")
        .into_inner();
    assert!(!first.resumed);
    let first_token = first.player_token.clone();

    // Second call with an unrelated (and previously-unseen) secret → fresh
    // seat, different token, resumed=false.
    let second = client
        .seat_player(Request::new(SeatPlayerRequest {
            name: "bob2".to_owned(),
            chips: 10_000,
            client_secret: "never-used-before".to_owned(),
        }))
        .await
        .expect("second seat")
        .into_inner();
    assert!(!second.resumed);
    assert_ne!(first_token, second.player_token, "different tokens");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seat_player_at_resume_returns_same_seat() {
    let (_guard, endpoint) = spawn_service().await;
    let mut client = DealerServiceClient::connect(endpoint)
        .await
        .expect("connect");

    let first = client
        .seat_player_at(Request::new(SeatPlayerAtRequest {
            seat: 3,
            name: "carol".to_owned(),
            chips: 10_000,
            client_secret: "carol-secret".to_owned(),
        }))
        .await
        .expect("first seat_at")
        .into_inner();
    assert!(matches!(
        first.result,
        Some(seat_player_at_response::Result::Success(true)),
    ));
    assert!(!first.resumed);
    let first_token = first.player_token.clone();

    let second = client
        .seat_player_at(Request::new(SeatPlayerAtRequest {
            seat: 3,
            name: "carol".to_owned(),
            chips: 10_000,
            client_secret: "carol-secret".to_owned(),
        }))
        .await
        .expect("second seat_at")
        .into_inner();
    assert!(matches!(
        second.result,
        Some(seat_player_at_response::Result::Success(true)),
    ));
    assert!(second.resumed);
    assert_eq!(first_token, second.player_token);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seat_player_at_resume_wrong_seat_returns_error() {
    let (_guard, endpoint) = spawn_service().await;
    let mut client = DealerServiceClient::connect(endpoint)
        .await
        .expect("connect");

    // Bind secret to seat 2.
    let _ = client
        .seat_player_at(Request::new(SeatPlayerAtRequest {
            seat: 2,
            name: "dave".to_owned(),
            chips: 10_000,
            client_secret: "dave-secret".to_owned(),
        }))
        .await
        .expect("first seat_at");

    // Try to resume the same secret at a *different* seat → error.
    let bad = client
        .seat_player_at(Request::new(SeatPlayerAtRequest {
            seat: 5,
            name: "dave".to_owned(),
            chips: 10_000,
            client_secret: "dave-secret".to_owned(),
        }))
        .await
        .expect("second seat_at")
        .into_inner();
    match bad.result {
        Some(seat_player_at_response::Result::Error(msg)) => {
            assert!(
                msg.contains("seat 2") || msg.contains("mismatch"),
                "error should mention the original seat or mismatch; got: {msg}",
            );
        }
        other => panic!("expected error, got {other:?}"),
    }
    assert!(!bad.resumed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_player_clears_secret_binding() {
    use pkdealer_proto::dealer::RemovePlayerRequest;

    let (_guard, endpoint) = spawn_service().await;
    let mut client = DealerServiceClient::connect(endpoint)
        .await
        .expect("connect");

    // Seat with a secret.
    let first = client
        .seat_player(Request::new(SeatPlayerRequest {
            name: "evict".to_owned(),
            chips: 10_000,
            client_secret: "evict-secret".to_owned(),
        }))
        .await
        .expect("first seat")
        .into_inner();
    let seat = match first.result {
        Some(seat_player_response::Result::SeatNumber(s)) => s,
        other => panic!("expected SeatNumber, got {other:?}"),
    };
    let first_token = first.player_token.clone();

    // Remove that seat.
    let _ = client
        .remove_player(Request::new(RemovePlayerRequest { seat }))
        .await
        .expect("remove");

    // Reseating with the same secret must allocate a *fresh* seat and token —
    // the previous binding was cleaned up.
    let second = client
        .seat_player(Request::new(SeatPlayerRequest {
            name: "evict-again".to_owned(),
            chips: 10_000,
            client_secret: "evict-secret".to_owned(),
        }))
        .await
        .expect("second seat")
        .into_inner();
    assert!(
        !second.resumed,
        "secret should have been cleared on removal"
    );
    assert_ne!(
        first_token, second.player_token,
        "fresh token after removal"
    );
}
