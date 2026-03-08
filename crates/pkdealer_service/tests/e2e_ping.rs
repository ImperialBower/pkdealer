use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command},
    time::{Duration, Instant},
};

use pkdealer_proto::{dealer::dealer_client::DealerClient, new_ping_request};

struct ChildProcessGuard {
    child: Child,
}

impl ChildProcessGuard {
    fn new(child: Child) -> Self {
        Self { child }
    }
}

impl Drop for ChildProcessGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn reserve_local_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("ephemeral listener should bind")
        .local_addr()
        .expect("ephemeral listener should provide local address")
        .port()
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root should be two levels above crate manifest")
        .to_path_buf()
}

fn service_bin_path() -> PathBuf {
    PathBuf::from(
        std::env::var("CARGO_BIN_EXE_pkdealer_service")
            .expect("cargo should provide path to pkdealer_service binary"),
    )
}

fn client_bin_path(service_bin_path: &Path) -> PathBuf {
    let client_name = if cfg!(windows) {
        "pkdealer_client.exe"
    } else {
        "pkdealer_client"
    };

    service_bin_path
        .parent()
        .expect("service binary should have a parent directory")
        .join(client_name)
}

fn ensure_client_binary(client_path: &Path) {
    if client_path.exists() {
        return;
    }

    let status = Command::new("cargo")
        .arg("build")
        .arg("-p")
        .arg("pkdealer_client")
        .arg("--bin")
        .arg("pkdealer_client")
        .current_dir(workspace_root())
        .status()
        .expect("cargo build should run to produce pkdealer_client binary");

    assert!(
        status.success(),
        "building pkdealer_client binary should succeed"
    );
}

async fn wait_for_service_ready(endpoint: &str, timeout: Duration) -> bool {
    let start = Instant::now();

    loop {
        let mut client = match DealerClient::connect(endpoint.to_owned()).await {
            Ok(client) => client,
            Err(_) if start.elapsed() < timeout => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            Err(_) => return false,
        };

        let request = tonic::Request::new(new_ping_request("readiness-check"));
        if client.ping(request).await.is_ok() {
            return true;
        }

        if start.elapsed() >= timeout {
            return false;
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn service_binary_and_client_binary_ping_round_trip() {
    let service_path = service_bin_path();
    let client_path = client_bin_path(&service_path);
    ensure_client_binary(&client_path);

    let port = reserve_local_port();
    let service_addr = format!("127.0.0.1:{port}");
    let endpoint = format!("http://{service_addr}");

    let service_child = Command::new(&service_path)
        .env("PKDEALER_ADDR", &service_addr)
        .spawn()
        .expect("pkdealer_service process should start");
    let _service_guard = ChildProcessGuard::new(service_child);

    let ready = wait_for_service_ready(&endpoint, Duration::from_secs(5)).await;
    assert!(ready, "service should become ready before timeout");

    let output = Command::new(&client_path)
        .env("PKDEALER_ENDPOINT", &endpoint)
        .env("PKDEALER_CLIENT_ID", "e2e-client")
        .output()
        .expect("pkdealer_client process should run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "client should exit successfully; stdout={stdout}; stderr={stderr}"
    );
    assert!(
        stdout.contains("Service response: pong:e2e-client"),
        "client output should include ping reply; stdout={stdout}; stderr={stderr}"
    );
}

#[tokio::test]
async fn service_binary_and_client_binary_ping_round_trip_empty_client_id() {
    let service_path = service_bin_path();
    let client_path = client_bin_path(&service_path);
    ensure_client_binary(&client_path);

    let port = reserve_local_port();
    let service_addr = format!("127.0.0.1:{port}");
    let endpoint = format!("http://{service_addr}");

    let service_child = Command::new(&service_path)
        .env("PKDEALER_ADDR", &service_addr)
        .spawn()
        .expect("pkdealer_service process should start");
    let _service_guard = ChildProcessGuard::new(service_child);

    let ready = wait_for_service_ready(&endpoint, Duration::from_secs(5)).await;
    assert!(ready, "service should become ready before timeout");

    let output = Command::new(&client_path)
        .env("PKDEALER_ENDPOINT", &endpoint)
        .env("PKDEALER_CLIENT_ID", "")
        .output()
        .expect("pkdealer_client process should run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "client should exit successfully; stdout={stdout}; stderr={stderr}"
    );
    assert!(
        stdout.contains("Service response: pong"),
        "client output should include pong reply; stdout={stdout}; stderr={stderr}"
    );
    assert!(
        !stdout.contains("Service response: pong:"),
        "client output should not include pong with client suffix; stdout={stdout}; stderr={stderr}"
    );
}
