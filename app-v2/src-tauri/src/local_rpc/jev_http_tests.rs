use super::*;
use crate::local_rpc::auth_fixture::Fixture;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
};
fn fixture() -> Fixture {
    let f = Fixture::new(json!({}));
    jev::save_key_with(
        &f.backend,
        "ready",
        "typesafe",
        "fake-jev-key",
        |_, _, _, _| Ok("verified"),
    )
    .unwrap();
    jev::select(&f.backend, "typesafe").unwrap();
    f
}
fn server(
    count: usize,
    status: u16,
    delay: Duration,
    slow_body: bool,
) -> (
    String,
    mpsc::Receiver<(u16, Value, String)>,
    thread::JoinHandle<()>,
) {
    let reply = json!({"answers":{"target":{"type":"choice","choice":"agent-a","probabilities":{"agent-a":1.0}}}}).to_string();
    server_with_reply(count, status, delay, slow_body, reply)
}
fn server_with_reply(
    count: usize,
    status: u16,
    delay: Duration,
    slow_body: bool,
    reply: String,
) -> (
    String,
    mpsc::Receiver<(u16, Value, String)>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (mut stream, peer) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        for _ in 0..count {
            let mut head = Vec::new();
            let mut byte = [0];
            while !head.ends_with(b"\r\n\r\n") {
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    return;
                }
                head.push(byte[0]);
            }
            let head = String::from_utf8(head).unwrap();
            let length = head
                .lines()
                .find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            let body = serde_json::from_slice(&body).unwrap();
            let _ = tx.send((peer.port(), body, head));
            if !slow_body {
                thread::sleep(delay);
            }
            if write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",reply.len()).is_err(){return}
            if slow_body {
                thread::sleep(delay);
            }
            if stream.write_all(reply.as_bytes()).is_err() {
                return;
            }
        }
    });
    (format!("http://127.0.0.1:{port}/v1"), rx, handle)
}
#[test]
fn requests_use_one_pooled_connection_and_native_channel_model_with_key_only_in_header() {
    let f = fixture();
    let credential = jev::current(&f.backend.owned).unwrap().unwrap();
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    let (url, rx, server) = server(2, 200, Duration::ZERO, false);
    for _ in 0..2 {
        request(
            &agent,
            &f.backend.owned,
            &credential,
            &url,
            json!({"state":{"new_message":{"text":"fixture"}},"questions":{}}),
            DEADLINE,
        )
        .unwrap();
    }
    let one = rx.recv().unwrap();
    let two = rx.recv().unwrap();
    assert_eq!(one.0, two.0, "reuse TCP connection");
    assert_eq!(one.1["model"], "jev-latest");
    assert!(one
        .2
        .to_lowercase()
        .contains("authorization: bearer fake-jev-key"));
    assert!(!one.1.to_string().contains("fake-jev-key"));
    server.join().unwrap();
}
#[test]
fn only_401_and_403_reject_shared_key_other_http_errors_do_not() {
    for status in [401, 403, 302, 500] {
        let f = fixture();
        let credential = jev::current(&f.backend.owned).unwrap().unwrap();
        let (url, rx, server) = server(1, status, Duration::ZERO, false);
        let error = request(
            &ureq::AgentBuilder::new().redirects(0).build(),
            &f.backend.owned,
            &credential,
            &url,
            json!({}),
            DEADLINE,
        )
        .unwrap_err();
        assert_eq!(
            error.code,
            if status >= 300 && status != 401 && status != 403 {
                "jev_unreachable"
            } else {
                "jev_auth"
            }
        );
        assert_eq!(
            jev::read(&f.backend.owned).unwrap().ready,
            status != 401 && status != 403
        );
        assert!(rx.recv().is_ok());
        server.join().unwrap();
    }
}
#[test]
fn a_total_deadline_covers_headers_and_body_without_retry_or_rejection() {
    for slow_body in [false, true] {
        let f = fixture();
        let credential = jev::current(&f.backend.owned).unwrap().unwrap();
        let (url, rx, server) = server(1, 200, Duration::from_millis(250), slow_body);
        let started = Instant::now();
        let error = request(
            &ureq::AgentBuilder::new().redirects(0).build(),
            &f.backend.owned,
            &credential,
            &url,
            json!({}),
            Duration::from_millis(80),
        )
        .unwrap_err();
        assert_eq!(error.code, "jev_timeout");
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(jev::read(&f.backend.owned).unwrap().ready);
        assert!(rx.recv().is_ok());
        server.join().unwrap();
        assert!(rx.try_recv().is_err());
    }
}
#[test]
fn oversized_or_non_json_200_responses_fail_without_retry_or_rejecting_the_key() {
    let oversized = json!({"padding": "x".repeat(MAX_REPLY as usize)}).to_string();
    assert!(oversized.len() as u64 > MAX_REPLY);
    for reply in [oversized, "not JSON".to_owned()] {
        let f = fixture();
        let credential = jev::current(&f.backend.owned).unwrap().unwrap();
        let (url, rx, server) = server_with_reply(1, 200, Duration::ZERO, false, reply);
        let error = request(
            &ureq::AgentBuilder::new().redirects(0).build(),
            &f.backend.owned,
            &credential,
            &url,
            json!({}),
            DEADLINE,
        )
        .unwrap_err();
        assert_eq!(error.code, "jev_unreachable");
        assert!(jev::read(&f.backend.owned).unwrap().ready);
        assert!(rx.recv().is_ok());
        server.join().unwrap();
        assert!(rx.try_recv().is_err(), "no classifier retry");
    }
}
#[test]
fn native_catalog_endpoints_match_the_pinned_classifier_models() {
    for (id, url, model) in [
        (
            "openrouter",
            "https://openrouter.ai/api/v1",
            "~typesafe/jev-latest",
        ),
        (
            "vercel-ai-gateway",
            "https://ai-gateway.vercel.sh/typesafe/v1",
            "typesafe-ai/jev",
        ),
        ("opencode", "https://opencode.ai/zen/v1", "jev-1.13"),
        ("typesafe", "https://api.typesafe.ai/v1", "jev-latest"),
    ] {
        assert_eq!(base_url(id), Some(url));
        assert_eq!(jev::model_id(id), Some(model));
    }
    assert_eq!(base_url("unknown"), None);
}
