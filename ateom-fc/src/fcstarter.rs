// Port of the Go ateom-fc's fcstarter.go: fork+exec firecracker, drain its
// stdout/stderr concurrently, then retry-poll its API socket until it
// reports healthy (or give up after HEALTH_CHECK_RETRIES).
//
// HTTP-over-Unix-socket goes through `reqwest`'s native Unix-socket support
// (`ClientBuilder::unix_socket`) — the socket path is baked into the
// `Client` once at construction (see `main::get_reqwest_http_client`), so
// every call here just targets a plain `http://localhost/...` URL.

use anyhow::{bail, Context, Result};
#[cfg(target_os = "linux")]
use nix::sched::{setns, CloneFlags};
use reqwest::{Client, StatusCode};
use std::fs::File;
use std::io::{BufRead, BufReader};
#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;

use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

pub(crate) const SOCKET_PATH: &str = "/tmp/firecracker.sock";
const HEALTH_CHECK_RETRIES: u32 = 5;
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(1);
#[cfg(target_os = "linux")]
pub async fn start(client: Client, actor_uuid: &str) -> Result<()> {
    // Remove any stale socket from a previous run, tolerating "didn't exist".
    if let Err(err) = std::fs::remove_file(SOCKET_PATH) {
        if err.kind() != std::io::ErrorKind::NotFound {
            return Err(err).context("removing stale firecracker socket");
        }
    }
    // we have to start firecracker in the net namespace as the tap .
    // this will require us to set current ns as the actors namespace
    // this should be done between fork and exec (right before we exec firecracker)
    // the command provides a pore_Exec hook which we will use
    let actor_ns = File::open(format!("{}{}", rtnetlink::NETNS_PATH, actor_uuid))?;
    // This assumes the Dockerfile adds firecracker as an executable.
    let mut fc_command = unsafe {
        Command::new("firecracker")
            .arg("--api-sock")
            .arg(SOCKET_PATH)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .pre_exec(move || {
                setns(&actor_ns, CloneFlags::CLONE_NEWNET)?;
                Ok(())
            })
            .spawn()
            .context("starting firecracker")?
    };

    let stdout = fc_command
        .stdout
        .take()
        .context("missing firecracker stdout")?;
    let stderr = fc_command
        .stderr
        .take()
        .context("missing firecracker stderr")?;


    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => println!("firecracker stdout {line}"),
                Err(err) => {
                    eprintln!("firecracker stdout read error: {err}");
                    break;
                }
            }
        }
    });
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            match line {
                Ok(line) => println!("firecracker stderr {line}"),
                Err(err) => {
                    eprintln!("firecracker stderr read error: {err}");
                    break;
                }
            }
        }
    });
    thread::spawn(move || match fc_command.wait() {
        Ok(status) if !status.success() => {
            eprintln!("firecracker command wait failed with status {status}");
        }
        Err(err) => eprintln!("firecracker command wait failed with error {err}"),
        _ => {}
    });

    is_fc_healthy(&client).await
}

async fn is_fc_healthy(client: &Client) -> Result<()> {
    for _ in 0..HEALTH_CHECK_RETRIES {
        match get_health(client).await {
            Ok(body) => {
                println!("body is {body}");
                return Ok(());
            }
            Err(err) => {
                eprintln!("firecracker err: {err}");
                tokio::time::sleep(HEALTH_CHECK_INTERVAL).await;
            }
        }
    }
    bail!("firecracker is not healthy")
}

async fn get_health(client: &Client) -> Result<String> {
    let response = client.get("http://localhost/").send().await?;
    if response.status() != StatusCode::OK {
        bail!("unexpected status: {}", response.status());
    }
    Ok(response.text().await?)
}
