use std::{
    io,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command},
    time::{Duration, Instant},
};

use pkdealer_proto::{dealer::dealer_service_client::DealerServiceClient, new_ping_request};

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

fn reserve_local_port() -> io::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

fn workspace_root() -> io::Result<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::other("workspace root should be two levels above crate manifest"))
}

fn service_bin_path() -> io::Result<PathBuf> {
    std::env::var("CARGO_BIN_EXE_pkdealer_service")
        .map(PathBuf::from)
        .map_err(|error| io::Error::new(io::ErrorKind::NotFound, error))
}

fn client_bin_path(service_bin_path: &Path) -> io::Result<PathBuf> {
    let client_name = if cfg!(windows) {
        "pkdealer_client.exe"
    } else {
        "pkdealer_client"
    };

    let parent = service_bin_path
        .parent()
        .ok_or_else(|| io::Error::other("service binary should have a parent directory"))?;

    Ok(parent.join(client_name))
}

fn ensure_client_binary(client_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if client_path.exists() {
        return Ok(());
    }

    let status = Command::new("cargo")
        .arg("build")
        .arg("-p")
        .arg("pkdealer_client")
        .arg("--bin")
        .arg("pkdealer_client")
        .current_dir(workspace_root()?)
        .status()?;

    assert!(
        status.success(),
        "building pkdealer_client binary should succeed"
    );

    Ok(())
}

async fn wait_for_service_ready(endpoint: &str, timeout: Duration) -> bool {
    let start = Instant::now();

    loop {
        let mut client = match DealerServiceClient::connect(endpoint.to_owned()).await {
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
async fn service_binary_and_client_binary_ping_round_trip() -> Result<(), Box<dyn std::error::Error>>
{
    let service_path = service_bin_path()?;
    let client_path = client_bin_path(&service_path)?;
    ensure_client_binary(&client_path)?;

    let port = reserve_local_port()?;
    let service_addr = format!("127.0.0.1:{port}");
    let endpoint = format!("http://{service_addr}");

    let service_child = Command::new(&service_path)
        .env("PKDEALER_ADDR", &service_addr)
        .spawn()?;
    let _service_guard = ChildProcessGuard::new(service_child);

    let ready = wait_for_service_ready(&endpoint, Duration::from_secs(5)).await;
    assert!(ready, "service should become ready before timeout");

    let output = Command::new(&client_path)
        .env("PKDEALER_ENDPOINT", &endpoint)
        .env("PKDEALER_CLIENT_ID", "e2e-client")
        .output()?;

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

    Ok(())
}

#[tokio::test]
async fn service_binary_and_client_binary_ping_round_trip_empty_client_id()
-> Result<(), Box<dyn std::error::Error>> {
    let service_path = service_bin_path()?;
    let client_path = client_bin_path(&service_path)?;
    ensure_client_binary(&client_path)?;

    let port = reserve_local_port()?;
    let service_addr = format!("127.0.0.1:{port}");
    let endpoint = format!("http://{service_addr}");

    let service_child = Command::new(&service_path)
        .env("PKDEALER_ADDR", &service_addr)
        .spawn()?;
    let _service_guard = ChildProcessGuard::new(service_child);

    let ready = wait_for_service_ready(&endpoint, Duration::from_secs(5)).await;
    assert!(ready, "service should become ready before timeout");

    let output = Command::new(&client_path)
        .env("PKDEALER_ENDPOINT", &endpoint)
        .env("PKDEALER_CLIENT_ID", "")
        .output()?;

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

    Ok(())
}
