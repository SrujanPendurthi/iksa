use crate::config::ModelConfig;
use anyhow::Context;
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Debug)]
pub struct Reply {
    pub tool_count: usize,
    pub tool: Option<(String, Value)>,
    pub text: String,
    pub stop_reason: String,
    pub usage: Value,
}

pub fn request_body(cfg: &ModelConfig, system: &str, tools: &Value, user: &str) -> Value {
    let mut tools = tools.clone();
    if let Some(tools) = tools.as_array_mut() {
        for tool in tools {
            tool["strict"] = json!(true);
            tool["input_schema"]["additionalProperties"] = json!(false);
        }
    }
    let mut body = json!({
        "model": cfg.name,
        "max_tokens": 4096,
        "thinking": {"type":"adaptive"},
        "output_config": {"effort":cfg.effort},
        "system": [{"type":"text","text":system,"cache_control":{"type":"ephemeral"}}],
        "tools": tools,
        "tool_choice": {"type":"auto","disable_parallel_tool_use":true},
        "messages": [{"role":"user","content":user}],
    });
    if cfg.fallbacks {
        body["fallbacks"] = json!("default");
    }
    body
}

fn connection_error(error: &ureq::Error) -> bool {
    matches!(
        error,
        ureq::Error::Io(_)
            | ureq::Error::Timeout(_)
            | ureq::Error::HostNotFound
            | ureq::Error::ConnectionFailed
    )
}

/// Blocking: the planner calls this on `spawn_blocking`.
pub fn call(cfg: &ModelConfig, key: &str, body: &Value) -> anyhow::Result<Reply> {
    // Never follow a redirect carrying the key or page content to another host.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(None)
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into();
    let url = format!("{}/v1/messages", cfg.endpoint.trim_end_matches('/'));
    for attempt in 0..3 {
        let mut request = agent
            .post(&url)
            .header("x-api-key", key)
            .header("anthropic-version", "2026-10-21")
            .header("content-type", "application/json");
        if cfg.fallbacks {
            request = request.header("anthropic-beta", "server-side-fallback-2026-07-01");
        }
        let mut response = match request.send_json(body) {
            Ok(response) => response,
            Err(error) => {
                if connection_error(&error) && attempt < 2 {
                    std::thread::sleep(Duration::from_secs(1 << attempt));
                    continue;
                }
                return Err(error).context("model API connection");
            }
        };
        let status = response.status().as_u16();
        if matches!(status, 429 | 500 | 502 | 503 | 529) && attempt < 2 {
            let seconds = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(1 << attempt)
                .min(30);
            drop(response);
            std::thread::sleep(Duration::from_secs(seconds));
            continue;
        }
        if !(200..300).contains(&status) {
            let error = response
                .body_mut()
                .read_json::<Value>()
                .unwrap_or(Value::Null);
            anyhow::bail!(
                "model API: {status} {}: {}",
                error["error"]["type"].as_str().unwrap_or("unknown_error"),
                error["error"]["message"]
                    .as_str()
                    .unwrap_or("invalid error response")
            );
        }
        let reply: Value = match response.body_mut().read_json() {
            Ok(reply) => reply,
            Err(error) if connection_error(&error) && attempt < 2 => {
                drop(response);
                std::thread::sleep(Duration::from_secs(1 << attempt));
                continue;
            }
            Err(error) => return Err(error).context("model API response JSON"),
        };
        let stop_reason = reply["stop_reason"]
            .as_str()
            .context("model API response missing stop_reason")?
            .to_owned();
        if stop_reason == "refusal" {
            anyhow::bail!(
                "model refused: {}",
                reply["stop_details"]["category"]
                    .as_str()
                    .unwrap_or("unknown")
            );
        }
        let blocks = reply["content"]
            .as_array()
            .context("model API response missing content")?;
        let tool = blocks
            .iter()
            .find(|b| b["type"] == "tool_use")
            .map(|b| {
                Ok::<_, anyhow::Error>((
                    b["name"]
                        .as_str()
                        .context("model API tool missing name")?
                        .to_owned(),
                    b.get("input")
                        .context("model API tool missing input")?
                        .clone(),
                ))
            })
            .transpose()?;
        let text = blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(Reply {
            tool_count: blocks.iter().filter(|b| b["type"] == "tool_use").count(),
            tool,
            text,
            stop_reason,
            usage: reply["usage"].clone(),
        });
    }
    unreachable!("the third attempt returns a response or error")
}

pub fn api_key() -> anyhow::Result<String> {
    if let Some(value) = std::env::var_os("ANTHROPIC_API_KEY") {
        let key = value
            .into_string()
            .map_err(|_| anyhow::anyhow!("run iksa key set"))?;
        anyhow::ensure!(!key.trim().is_empty(), "run iksa key set");
        return Ok(key);
    }
    let key = crate::keychain::get("iksa", "anthropic").context("run iksa key set")?;
    anyhow::ensure!(!key.is_empty(), "run iksa key set");
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ModelConfig;
    use serde_json::{Value, json};
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Duration,
    };

    struct Mock {
        cfg: ModelConfig,
        requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }
    impl Mock {
        fn new(responses: Vec<(u16, &str, Value)>) -> Self {
            let responses: Vec<_> = responses
                .into_iter()
                .map(|(s, h, b)| (s, h.to_owned(), b))
                .collect();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let (seen, done) = (requests.clone(), stop.clone());
            let worker = thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(s) => s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(e) => panic!("mock accept: {e}"),
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut reader = BufReader::new(&mut stream);
                    let mut request = String::new();
                    loop {
                        let mut line = String::new();
                        assert!(reader.read_line(&mut line).unwrap() > 0);
                        request.push_str(&line);
                        if line == "\r\n" {
                            break;
                        }
                    }
                    let length = request
                        .lines()
                        .find_map(|l| {
                            l.split_once(':')
                                .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                                .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    request.push_str(std::str::from_utf8(&body).unwrap());
                    let mut requests = seen.lock().unwrap();
                    let index = requests.len();
                    requests.push(request);
                    drop(requests);
                    let (status, headers, body) = &responses[index.min(responses.len() - 1)];
                    // Status 0 simulates a connection closing partway through a 200 body.
                    if *status == 0 {
                        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n{").unwrap();
                        continue;
                    }
                    let body = body.to_string();
                    write!(stream, "HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}", body.len()).unwrap();
                }
            });
            let mut cfg = config();
            cfg.endpoint = endpoint;
            Self {
                cfg,
                requests,
                stop,
                worker: Some(worker),
            }
        }
        fn count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }
    impl Drop for Mock {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let result = self.worker.take().unwrap().join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
    }
    fn config() -> ModelConfig {
        ModelConfig {
            provider: "anthropic".into(),
            endpoint: "https://api.anthropic.com".into(),
            name: "test-model".into(),
            effort: "low".into(),
            max_steps: 25,
            fallbacks: true,
        }
    }
    fn success() -> Value {
        json!({"content":[{"type":"thinking","thinking":"private reasoning"},{"type":"text","text":"first"},{"type":"tool_use","id":"t1","name":"click","input":{"ref":7}},{"type":"text","text":"second"},{"type":"tool_use","id":"t2","name":"done","input":{"answer":"ignored"}}],"stop_reason":"tool_use","usage":{"input_tokens":10,"output_tokens":5}})
    }
    fn error(kind: &str) -> Value {
        json!({"error":{"type":kind,"message":"fixture error"}})
    }

    #[test]
    fn request_body_shape() {
        let mut cfg = config();
        let tools = json!([{"name":"click","description":"Click","input_schema":{"type":"object","properties":{"ref":{"type":"integer"}},"required":["ref"]}}]);
        let body = request_body(&cfg, "system text", &tools, "goal");
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(body["thinking"], json!({"type":"adaptive"}));
        assert_eq!(body["output_config"], json!({"effort":"low"}));
        assert_eq!(
            body["tool_choice"],
            json!({"type":"auto","disable_parallel_tool_use":true})
        );
        assert_eq!(
            body["system"],
            json!([{"type":"text","text":"system text","cache_control":{"type":"ephemeral"}}])
        );
        assert_eq!(body["messages"], json!([{"role":"user","content":"goal"}]));
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(body["tools"][0]["strict"], true);
        assert_eq!(
            body["tools"][0]["input_schema"]["additionalProperties"],
            false
        );
        assert!(tools[0].get("strict").is_none());
        for forbidden in ["temperature", "top_p", "top_k", "budget_tokens"] {
            assert!(body.get(forbidden).is_none());
        }
        cfg.fallbacks = false;
        assert!(
            request_body(&cfg, "s", &tools, "u")
                .get("fallbacks")
                .is_none()
        );
        let mut mock = Mock::new(vec![(200, "", success())]);
        mock.cfg.fallbacks = false;
        call(&mock.cfg, "test-key", &json!({})).unwrap();
        assert!(
            !mock.requests.lock().unwrap()[0]
                .to_ascii_lowercase()
                .contains("anthropic-beta:")
        );
    }
    #[test]
    fn parses_tool_use() {
        let mock = Mock::new(vec![(200, "", success())]);
        let reply = call(&mock.cfg, "test-key", &json!({})).unwrap();
        assert_eq!(reply.tool, Some(("click".into(), json!({"ref":7}))));
        assert_eq!(reply.text, "first\nsecond");
        assert_eq!(reply.stop_reason, "tool_use");
        assert_eq!(reply.usage, json!({"input_tokens":10,"output_tokens":5}));
    }
    #[test]
    fn retries_on_429_then_succeeds() {
        let mock = Mock::new(vec![
            (429, "retry-after: 0\r\n", error("rate_limit_error")),
            (200, "", success()),
        ]);
        call(&mock.cfg, "test-key", &json!({})).unwrap();
        assert_eq!(mock.count(), 2);
    }
    #[test]
    fn auth_error_not_retried() {
        for status in [400, 401, 403, 404] {
            let mock = Mock::new(vec![(status, "", error("authentication_error"))]);
            let err = call(&mock.cfg, "test-key", &json!({}))
                .unwrap_err()
                .to_string();
            assert_eq!(
                err,
                format!("model API: {status} authentication_error: fixture error")
            );
            assert_eq!(mock.count(), 1);
        }
    }
    #[test]
    fn refusal_is_error() {
        let mock = Mock::new(vec![(
            200,
            "",
            json!({"content":[],"stop_reason":"refusal","stop_details":{"category":"cyber"},"usage":{}}),
        )]);
        let err = call(&mock.cfg, "test-key", &json!({}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("refused: cyber"), "{err}");
    }
    #[test]
    fn headers_sent() {
        let mock = Mock::new(vec![(200, "", success())]);
        call(&mock.cfg, "test-key", &json!({"hello":"world"})).unwrap();
        let requests = mock.requests.lock().unwrap();
        let request = requests[0].to_ascii_lowercase();
        for expected in [
            "post /v1/messages http/1.1",
            "x-api-key: test-key\r\n",
            "anthropic-version: 2026-10-21\r\n",
            "anthropic-beta: server-side-fallback-2026-07-01\r\n",
            "content-type: application/json\r\n",
        ] {
            assert!(request.contains(expected), "missing {expected}: {request}");
        }
        assert_eq!(
            serde_json::from_str::<Value>(request.split_once("\r\n\r\n").unwrap().1).unwrap(),
            json!({"hello":"world"})
        );
    }
    #[test]
    fn retryable_statuses_stop_after_three_attempts() {
        for status in [429, 500, 502, 503, 529] {
            let mock = Mock::new(vec![(
                status,
                "retry-after: 0\r\n",
                error("overloaded_error"),
            )]);
            let err = call(&mock.cfg, "test-key", &json!({}))
                .unwrap_err()
                .to_string();
            assert_eq!(
                err,
                format!("model API: {status} overloaded_error: fixture error")
            );
            assert_eq!(mock.count(), 3);
        }
    }
    #[test]
    fn redirects_are_not_followed() {
        let destination = Mock::new(vec![(200, "", success())]);
        let header = format!("Location: {}/v1/messages\r\n", destination.cfg.endpoint);
        let source = Mock::new(vec![(302, &header, json!({}))]);
        assert!(call(&source.cfg, "test-key", &json!({})).is_err());
        assert_eq!(source.count(), 1);
        assert_eq!(destination.count(), 0);
    }
    #[test]
    fn disconnected_response_body_is_retried() {
        let mock = Mock::new(vec![(0, "", Value::Null), (200, "", success())]);
        let reply = call(&mock.cfg, "test-key", &json!({})).unwrap();
        assert_eq!(reply.tool, Some(("click".into(), json!({"ref":7}))));
        assert_eq!(mock.count(), 2);
    }
}
