use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn request(port: u16, data: &[u8]) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    stream.write_all(data).unwrap();
    let mut answer = [0; 512];
    let n = stream.read(&mut answer).unwrap();
    String::from_utf8_lossy(&answer[..n]).into_owned()
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn slow_delete_does_not_block_health_and_untrusted_delete_is_rejected_before_body() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let child = Server(
        Command::new(env!("CARGO_BIN_EXE_github-finds"))
            .args([
                "serve",
                "--port",
                &port.to_string(),
                "--trusted",
                "127.0.0.1",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut slow = TcpStream::connect(("127.0.0.1", port)).unwrap();
    slow.write_all(b"POST /remove HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 2048\r\n\r\n").unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let start = Instant::now();
    assert!(request(port, b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n").contains("200 OK"));
    assert!(start.elapsed() < Duration::from_secs(1));
    let mut stalled = Vec::new();
    for _ in 0..15 {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(b"POST /remove HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 2048\r\n\r\n").unwrap();
        stalled.push(stream);
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(request(port, b"POST /remove HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 2048\r\n\r\n").contains("503 Service Unavailable"));
    assert!(request(port, b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n").contains("200 OK"));
    assert!(request(port, b"POST /missing HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 2048\r\n\r\n").contains("404 Not Found"));
    assert!(request(port, b"POST /remove HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/plain\r\nContent-Length: 2048\r\n\r\n").contains("415 Unsupported Media Type"));
    drop(stalled);
    drop(child);

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let other_port = listener.local_addr().unwrap().port();
    drop(listener);
    let other = Server(
        Command::new(env!("CARGO_BIN_EXE_github-finds"))
            .args(["serve", "--port", &other_port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", other_port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(request(other_port, b"POST /remove HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 2048\r\n\r\n").contains("403 Forbidden"));
    drop(other);
}

#[test]
fn trusted_delete_accepts_a_bounded_chunked_body() {
    let dir = std::env::temp_dir().join(format!("finds-http-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let links = dir.join("links.json");
    let state = dir.join("discover.json");
    let url = "https://github.com/o/r";
    let listing = format!(r#"[{{"url":"{url}","title":"o/r","why":"why","added":"2026-10-01"}}]"#);
    std::fs::write(&links, &listing).unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let child = Server(
        Command::new(env!("CARGO_BIN_EXE_github-finds"))
            .args([
                "serve",
                "--port",
                &port.to_string(),
                "--trusted",
                "127.0.0.1",
                "--links",
                links.to_str().unwrap(),
                "--state",
                state.to_str().unwrap(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let body = format!(r#"{{"url":"{url}"}}"#);
    let payload = format!(
        "POST /remove HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n{:X}\r\n{}\r\n0\r\n\r\n",
        body.len(),
        body
    );
    assert!(request(port, payload.as_bytes()).contains("200 OK"));
    assert_eq!(std::fs::read_to_string(&links).unwrap().trim(), "[]");
    assert!(std::fs::read_to_string(&state).unwrap().contains(url));
    std::fs::write(&links, listing).unwrap();
    let payload = format!(
        "POST /remove HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    assert!(request(port, payload.as_bytes()).contains("200 OK"));
    assert_eq!(std::fs::read_to_string(&links).unwrap().trim(), "[]");
    drop(child);
    std::fs::remove_file(links).unwrap();
    std::fs::remove_file(state).unwrap();
    std::fs::remove_file(dir.join(".finds.lock")).unwrap();
    std::fs::remove_dir(dir).unwrap();
}
