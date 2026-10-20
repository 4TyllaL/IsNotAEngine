//! HTTP dos provedores de IA: POST/GET em JSON com repetição em falhas passageiras.
//! Nunca registra nem devolve a chave de API nem o corpo da requisição.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;

use super::provider::Request;

/// Respostas longas (um 4K com imagens no histórico) demoram: o tempo total é generoso.
const TIMEOUT: Duration = Duration::from_secs(600);
/// Esperas entre tentativas em 429/5xx/overload.
pub const DEFAULT_RETRY: [Duration; 3] = [Duration::from_secs(2), Duration::from_secs(6), Duration::from_secs(15)];

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        // Os provedores respondem direto; seguir um redirecionamento levaria o cabeçalho com a chave para outro host.
        .max_redirects(0)
        // Queremos o corpo dos 4xx/5xx: é onde o provedor explica o erro.
        .http_status_as_error(false)
        .build()
        .into()
}

fn retryable(status: u16) -> bool {
    matches!(status, 408 | 409 | 425 | 429 | 500 | 502 | 503 | 504 | 529)
}

/// Espera `d` em fatias, acordando se o usuário cancelar. Devolve `false` se foi cancelado.
fn sleep_unless_cancelled(d: Duration, cancel: &AtomicBool) -> bool {
    let end = std::time::Instant::now() + d;
    while std::time::Instant::now() < end {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    !cancel.load(Ordering::Relaxed)
}

fn friendly_status(status: u16, provider_message: Option<String>) -> String {
    let base = match status {
        401 | 403 => "the API key was rejected (check that it is correct and has access to this model)".to_string(),
        404 => "model or endpoint not found (check the model name)".to_string(),
        413 => "the request was too large".to_string(),
        429 => "rate limit or quota reached; try again in a moment (free tiers have strict limits)".to_string(),
        s if s >= 500 => "the provider is having trouble; try again later".to_string(),
        s => format!("the provider answered HTTP {s}"),
    };
    match provider_message {
        Some(m) if !m.is_empty() => format!("{base}: {m}"),
        _ => base,
    }
}

fn send(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&Value>,
    retry: &[Duration],
    cancel: &AtomicBool,
) -> Result<Value, String> {
    // Serializa aqui (compacto): o `send_json` do ureq indenta, e com imagens em base64 isso só incha.
    let payload = match body {
        Some(b) => serde_json::to_vec(b).map_err(|e| format!("could not encode the request: {e}"))?,
        None => Vec::new(),
    };
    let mut attempt = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let net = match exchange_cancellable(method, url, headers, &payload, cancel) {
            Some(n) => n,
            None => return Err("cancelled".into()),
        };
        match net {
            Net::Reply(status, text) => {
                let parsed: Result<Value, _> = serde_json::from_str(&text);
                if (200..300).contains(&status) {
                    return parsed.map_err(|_| "the provider returned a reply that is not JSON".to_string());
                }
                if retryable(status)
                    && let Some(wait) = retry.get(attempt)
                {
                    attempt += 1;
                    if !sleep_unless_cancelled(*wait, cancel) {
                        return Err("cancelled".into());
                    }
                    continue;
                }
                let message = parsed.ok().as_ref().and_then(super::provider::api_error);
                return Err(friendly_status(status, message));
            }
            Net::ReadFailed(e) => return Err(e),
            Net::Unreachable(e) => {
                // Falha de rede (DNS, conexão, timeout): também vale repetir.
                if let Some(wait) = retry.get(attempt) {
                    attempt += 1;
                    if !sleep_unless_cancelled(*wait, cancel) {
                        return Err("cancelled".into());
                    }
                    continue;
                }
                return Err(format!("could not reach the provider: {e}"));
            }
        }
    }
}

/// O que veio de uma troca HTTP.
enum Net {
    Reply(u16, String),
    ReadFailed(String),
    Unreachable(String),
}

/// Uma requisição, bloqueante (roda numa thread própria: ver `exchange_cancellable`).
fn exchange(method: &str, url: &str, headers: &[(String, String)], payload: &[u8]) -> Net {
    let agent = agent();
    let result = if method == "POST" {
        let mut req = agent.post(url);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        req.send(payload)
    } else {
        let mut req = agent.get(url);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        req.call()
    };
    match result {
        Ok(mut resp) => {
            let status = resp.status().as_u16();
            match resp.body_mut().read_to_string() {
                Ok(text) => Net::Reply(status, text),
                Err(e) => Net::ReadFailed(format!("could not read the reply: {e}")),
            }
        }
        Err(e) => Net::Unreachable(e.to_string()),
    }
}

/// Faz a troca numa thread e espera olhando o cancelamento a cada 100 ms: uma resposta de minutos
/// (o modelo escrevendo uma página grande) não segura mais o "Cancelar". `None` = cancelado.
/// A thread termina sozinha quando o provedor responder ou o tempo limite vencer; o resultado é descartado.
fn exchange_cancellable(method: &str, url: &str, headers: &[(String, String)], payload: &[u8], cancel: &AtomicBool) -> Option<Net> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (method, url, headers, payload) = (method.to_string(), url.to_string(), headers.to_vec(), payload.to_vec());
    std::thread::spawn(move || {
        let _ = tx.send(exchange(&method, &url, &headers, &payload));
    });
    loop {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(net) => return Some(net),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Some(Net::Unreachable("the request thread stopped".into())),
        }
    }
}

pub fn post_json(req: &Request, retry: &[Duration], cancel: &AtomicBool) -> Result<Value, String> {
    send("POST", &req.url, &req.headers, Some(&req.body), retry, cancel)
}

pub fn get_json(url: &str, headers: &[(String, String)], cancel: &AtomicBool) -> Result<Value, String> {
    send("GET", url, headers, None, &DEFAULT_RETRY[..1], cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// Servidor HTTP mínimo: responde cada conexão com a próxima `(status, corpo)` e guarda o que recebeu.
    fn mock(replies: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/x", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for (status, body) in replies {
                let Ok((mut s, _)) = listener.accept() else { return };
                let mut buf = vec![0u8; 65536];
                let mut got = Vec::new();
                // lê cabeçalhos + corpo (Content-Length)
                loop {
                    let n = s.read(&mut buf).unwrap_or(0);
                    got.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&got).to_string();
                    if let Some(h) = text.find("\r\n\r\n") {
                        let len = text[..h]
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        if got.len() >= h + 4 + len || n == 0 {
                            break;
                        }
                    } else if n == 0 {
                        break;
                    }
                }
                log.lock().unwrap().push(String::from_utf8_lossy(&got).to_string());
                let reply = format!("HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                let _ = s.write_all(reply.as_bytes());
            }
        });
        (url, seen)
    }

    fn req(url: &str) -> Request {
        Request { url: url.into(), headers: vec![("x-api-key".into(), "SEGREDO".into())], body: serde_json::json!({"oi": 1}) }
    }

    const FAST: [Duration; 3] = [Duration::from_millis(10), Duration::from_millis(10), Duration::from_millis(10)];

    #[test]
    fn retries_overload_then_succeeds_and_sends_headers_and_body() {
        let (url, seen) = mock(vec![(529, r#"{"error":{"message":"overloaded"}}"#.into()), (429, "{}".into()), (200, r#"{"ok":true}"#.into())]);
        let out = post_json(&req(&url), &FAST, &AtomicBool::new(false)).expect("terceira tentativa passa");
        assert_eq!(out["ok"], true);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        let last = seen[2].to_ascii_lowercase();
        assert!(last.contains("x-api-key: segredo") && last.contains("\"oi\":1") && last.starts_with("post /v1/x"), "{last}");
    }

    #[test]
    fn auth_errors_are_not_retried_and_explain_themselves_without_leaking_the_key() {
        let (url, seen) = mock(vec![(401, r#"{"error":{"message":"invalid x-api-key"}}"#.into()), (200, "{}".into())]);
        let err = post_json(&req(&url), &FAST, &AtomicBool::new(false)).unwrap_err();
        assert!(err.contains("API key was rejected") && err.contains("invalid x-api-key"), "{err}");
        assert!(!err.contains("SEGREDO"));
        assert_eq!(seen.lock().unwrap().len(), 1, "401 não repete");
    }

    #[test]
    fn gives_up_after_the_retries_and_reports_rate_limits() {
        let (url, seen) = mock(vec![(429, "{}".into()); 4]);
        let err = post_json(&req(&url), &FAST, &AtomicBool::new(false)).unwrap_err();
        assert!(err.contains("rate limit"), "{err}");
        assert_eq!(seen.lock().unwrap().len(), 4, "1 tentativa + 3 repetições");
    }

    #[test]
    fn cancel_stops_before_any_request() {
        let (url, seen) = mock(vec![(200, "{}".into())]);
        assert_eq!(post_json(&req(&url), &FAST, &AtomicBool::new(true)).unwrap_err(), "cancelled");
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn non_json_replies_and_dead_servers_are_errors_not_panics() {
        let (url, _) = mock(vec![(200, "<html>nope</html>".into())]);
        assert!(post_json(&req(&url), &[], &AtomicBool::new(false)).unwrap_err().contains("not JSON"));
        assert!(post_json(&req("http://127.0.0.1:1/x"), &[], &AtomicBool::new(false)).unwrap_err().contains("could not reach"));
    }

    #[test]
    fn get_works_for_model_lists() {
        let (url, seen) = mock(vec![(200, r#"{"data":[]}"#.into())]);
        let out = get_json(&url, &[("authorization".into(), "Bearer K".into())], &AtomicBool::new(false)).unwrap();
        assert!(out["data"].is_array());
        assert!(seen.lock().unwrap()[0].starts_with("GET /v1/x"));
    }

    #[test]
    fn cancel_does_not_wait_for_a_slow_reply() {
        // um servidor que aceita e fica calado por 5 s (o modelo "escrevendo uma página grande")
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut c, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = c.read(&mut buf);
                std::thread::sleep(Duration::from_secs(5));
            }
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::SeqCst);
        });
        let started = std::time::Instant::now();
        let r = send("POST", &url, &[], Some(&serde_json::json!({"a": 1})), &[], &cancel);
        assert_eq!(r.unwrap_err(), "cancelled");
        assert!(started.elapsed() < Duration::from_secs(2), "cancelou em {:?}", started.elapsed());
    }
}
