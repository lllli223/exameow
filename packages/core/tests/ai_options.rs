use exameow_core::ai::{AIClient, AIRequestOptions};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

fn options_from(value: Value) -> AIRequestOptions {
    serde_json::from_value(value).unwrap()
}

#[test]
fn defaults_are_backwards_compatible() {
    let options: AIRequestOptions = serde_json::from_value(json!({})).unwrap();
    assert_eq!(options.token_parameter, "max_tokens");
    assert_eq!(options.retries, 0);
    assert!(!options.omit_temperature);
    assert!(options.max_tokens.is_none());
}

#[test]
fn validation_rejects_out_of_range_values() {
    assert!(options_from(json!({ "retries": 6 })).validate().is_err());
    assert!(options_from(json!({ "max_tokens": 0 })).validate().is_err());
    assert!(options_from(json!({ "timeout_seconds": 0 })).validate().is_err());
    assert!(options_from(json!({ "temperature": 3.0 })).validate().is_err());
    assert!(options_from(json!({ "reasoning_effort": "ultra" })).validate().is_err());
    assert!(options_from(json!({ "token_parameter": "max_output_tokens" })).validate().is_err());
    assert!(options_from(json!({ "retries": 5, "max_tokens": 1000 })).validate().is_ok());
}

#[test]
fn apply_maps_fields_and_finds_system_message() {
    let options = options_from(json!({
        "max_tokens": 2048,
        "token_parameter": "max_completion_tokens",
        "omit_temperature": true,
        "reasoning_effort": "high",
        "extra_prompt": "Answer in bullet points"
    }));
    let mut body = json!({
        "model": "m",
        "messages": [
            {"role": "user", "content": "u"},
            {"role": "system", "content": "SYS"}
        ],
        "temperature": 0.7,
        "max_tokens": 16384
    });
    options.apply(&mut body);

    assert_eq!(body["max_completion_tokens"], 2048);
    assert!(body.get("max_tokens").is_none());
    assert!(body.get("temperature").is_none());
    assert_eq!(body["reasoning_effort"], "high");
    let system = body["messages"][1]["content"].as_str().unwrap();
    assert!(system.starts_with("SYS"));
    assert!(system.contains("Answer in bullet points"));
}

#[test]
fn apply_without_max_tokens_keeps_caller_value() {
    let options = options_from(json!({ "temperature": 0.2 }));
    let mut body = json!({
        "messages": [{"role": "system", "content": "s"}],
        "max_tokens": 16384
    });
    options.apply(&mut body);
    assert_eq!(body["max_tokens"], 16384);
    assert_eq!(body["temperature"], 0.2);
}

fn spawn_server(statuses: Vec<u16>) -> (String, Arc<Mutex<Vec<Value>>>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let received = requests.clone();
    let handle = thread::spawn(move || {
        for status in statuses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let n = stream.read(&mut buffer).unwrap();
                assert!(n > 0, "client closed before request body");
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                    let len: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if bytes.len() >= end + 4 + len {
                        received
                            .lock()
                            .unwrap()
                            .push(serde_json::from_slice(&bytes[end + 4..end + 4 + len]).unwrap());
                        break;
                    }
                }
            }
            let body = json!({"choices":[{"message":{"content":"ok"}}]}).to_string();
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            let _ = stream.write_all(body.as_bytes());
        }
    });
    (endpoint, requests, handle)
}

#[tokio::test]
async fn retries_transient_errors_with_identical_body() {
    let (endpoint, requests, handle) = spawn_server(vec![503, 200]);
    let options = options_from(json!({
        "retries": 1,
        "reasoning_effort": "high",
        "max_tokens": 512,
        "token_parameter": "max_completion_tokens"
    }));
    let client = AIClient::new(&endpoint, "test").with_options(Some(options)).unwrap();
    assert_eq!(
        client.chat("Return JSON", "Question", "test-model").await.unwrap(),
        "ok"
    );
    handle.join().unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    assert_eq!(requests[0]["reasoning_effort"], "high");
    assert_eq!(requests[0]["max_completion_tokens"], 512);
    assert!(requests[0].get("max_tokens").is_none());
}

#[tokio::test]
async fn does_not_retry_client_errors() {
    let (endpoint, requests, handle) = spawn_server(vec![400]);
    let options = options_from(json!({ "retries": 3 }));
    let client = AIClient::new(&endpoint, "test").with_options(Some(options)).unwrap();
    assert!(client.chat("s", "u", "m").await.is_err());
    handle.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), 1);
}
