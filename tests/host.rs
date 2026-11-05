use agent_net::host::{self, HostOptions, HostRequest};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::AsyncReadExt;

fn options(dir: PathBuf, id: &str) -> HostOptions {
    // An interactive shell that echoes what it is sent.
    let (command, args): (PathBuf, Vec<String>) = if cfg!(windows) {
        ("cmd.exe".into(), vec!["/q".into(), "/k".into(), "echo READY".into()])
    } else {
        ("/bin/sh".into(), vec!["-c".into(), "echo READY; while read line; do echo got-$line; done".into()])
    };
    HostOptions { id: id.into(), command, args, cwd: std::env::temp_dir(), dir, env: HashMap::new(), scrollback: None }
}

async fn read_until(stream: &mut Box<dyn host::Stream>, needle: &str) -> String {
    let mut seen = String::new();
    let mut chunk = [0u8; 4096];
    let found = tokio::time::timeout(Duration::from_secs(20), async {
        while !seen.contains(needle) {
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0, "stream closed before {needle:?}: {seen}");
            seen.push_str(&String::from_utf8_lossy(&chunk[..n]));
        }
    })
    .await;
    assert!(found.is_ok(), "timed out waiting for {needle:?}: {seen}");
    seen
}

#[tokio::test]
async fn host_runs_attaches_and_stops() {
    let dir = tempfile::tempdir().unwrap();
    let id = format!("test-{}", std::process::id());
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_agent-net"));
    host::start_host(&exe, &options(dir.path().to_path_buf(), &id)).await.unwrap();
    assert!(host::is_running(dir.path(), &id).await);

    let mut stream = host::attach(dir.path(), &id, 100, 30).await.unwrap();
    read_until(&mut stream, "READY").await;
    let input = if cfg!(windows) { "echo got-%CD:~0,0%ping\r" } else { "ping\n" };
    host::send(dir.path(), &id, &HostRequest::Write { data: input.into() }).await.unwrap();
    read_until(&mut stream, "got-ping").await;

    host::send(dir.path(), &id, &HostRequest::Stop).await.unwrap();
    for _ in 0..100 {
        if !host::is_running(dir.path(), &id).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("host did not stop");
}
