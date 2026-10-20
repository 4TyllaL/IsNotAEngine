//! Compilação de shaders em processo filho encerrável (contrato em `docs/compiler-worker.md`).
//!
//! O `D3DCompile` não pode ser cancelado dentro do processo; num processo filho, o timeout
//! realmente mata a compilação. O filho é o próprio executável com `--compile-worker`: não abre
//! desktop, config, credenciais nem rede.

use std::io::{BufRead, Read, Write};
use std::process::{Command, Stdio};
use std::sync::{Condvar, Mutex, mpsc};
use std::time::Duration;

use serde_json::{Value, json};

use crate::render::pipeline::{CompiledShader, compile_uncached};

pub const PROTOCOL_VERSION: u64 = 1;
/// Mesmo limite do contrato de shader.
pub const MAX_SOURCE_BYTES: usize = 256 * 1024;
const MAX_REQUEST_BYTES: u64 = 300 * 1024;
const MAX_RESPONSE_BYTES: u64 = 8 << 20;
const MAX_ACTIVE: usize = 2;
const MAX_WAITING: usize = 4;

#[derive(Debug, PartialEq, Eq)]
pub enum WorkerError {
    /// O filho passou do prazo e foi encerrado.
    Timeout,
    /// Fila cheia: o pedido nem começou.
    Busy,
    /// Pedido recusado pelo worker (limite, versão).
    Rejected(String),
    /// O filho morreu, não respondeu ou respondeu lixo.
    Crashed(String),
}

// ---------------------------------------------------------------- worker (processo filho)

/// Ponto de entrada de `--compile-worker`: um pedido do stdin, uma resposta no stdout.
pub fn worker_main() -> i32 {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    serve(&mut stdin.lock(), &mut out)
}

fn serve<R: BufRead, W: Write>(input: &mut R, out: &mut W) -> i32 {
    let mut line = Vec::new();
    // Lê um byte a mais que o teto para distinguir "exatamente no limite" de "passou".
    if input.take(MAX_REQUEST_BYTES + 1).read_until(b'\n', &mut line).is_err() {
        return 64;
    }
    let response = if line.len() as u64 > MAX_REQUEST_BYTES {
        error_response("rejected", format!("request is larger than {} KiB", MAX_REQUEST_BYTES / 1024))
    } else {
        handle(&line)
    };
    if writeln!(out, "{response}").and_then(|_| out.flush()).is_err() {
        return 64;
    }
    0
}

fn error_response(kind: &str, error: String) -> Value {
    json!({"v": PROTOCOL_VERSION, "ok": false, "kind": kind, "error": error})
}

fn handle(line: &[u8]) -> Value {
    // Alguns clientes (ex.: .NET Framework / PowerShell 5.1) escrevem um BOM UTF-8 no início do stdin.
    let line = line.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(line);
    let Ok(req) = serde_json::from_slice::<Value>(line) else {
        return error_response("protocol", "invalid JSON request".into());
    };
    if req.get("v").and_then(Value::as_u64) != Some(PROTOCOL_VERSION) {
        return error_response("rejected", format!("unsupported protocol version (expected {PROTOCOL_VERSION})"));
    }
    let (Some(name), Some(source)) = (req.get("name").and_then(Value::as_str), req.get("source").and_then(Value::as_str)) else {
        return error_response("protocol", "`name` and `source` must be strings".into());
    };
    if source.len() > MAX_SOURCE_BYTES {
        return error_response(
            "rejected",
            format!("source is {} KiB; the limit is {} KiB", source.len() / 1024, MAX_SOURCE_BYTES / 1024),
        );
    }
    let started = std::time::Instant::now();
    match compile_uncached(source, name) {
        Ok(c) => json!({
            "v": PROTOCOL_VERSION, "ok": true,
            "vs": b64_encode(&c.vs), "ps": b64_encode(&c.ps),
            "messages": c.messages, "ms": started.elapsed().as_secs_f64() * 1000.0,
        }),
        Err(msg) => error_response("compile", msg),
    }
}

// ---------------------------------------------------------------- cliente (processo principal)

/// Compila `source` num processo filho. `Ok(Err(texto))` é erro de compilação do shader.
pub fn compile(source: &str, name: &str, timeout: Duration) -> Result<Result<CompiledShader, String>, WorkerError> {
    #[cfg(test)]
    {
        // Em testes `current_exe` é o executável de teste, não o app: compila no processo.
        let _ = timeout;
        Ok(compile_uncached(source, name))
    }
    #[cfg(not(test))]
    {
        let exe = std::env::current_exe().map_err(|e| WorkerError::Crashed(format!("current_exe: {e}")))?;
        let mut cmd = Command::new(exe);
        cmd.arg("--compile-worker");
        compile_with(cmd, source, name, timeout)
    }
}

/// Executa `cmd` como worker. Separado de [`compile`] para os testes usarem workers falsos.
fn compile_with(cmd: Command, source: &str, name: &str, timeout: Duration) -> Result<Result<CompiledShader, String>, WorkerError> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(WorkerError::Rejected(format!(
            "source is {} KiB; the limit is {} KiB",
            source.len() / 1024,
            MAX_SOURCE_BYTES / 1024
        )));
    }
    let _slot = Slot::acquire()?;
    let request = json!({"v": PROTOCOL_VERSION, "name": name, "source": source}).to_string();
    let raw = run_child(cmd, &request, timeout)?;
    parse_response(&raw)
}

fn run_child(mut cmd: Command, request: &str, timeout: Duration) -> Result<Vec<u8>, WorkerError> {
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).env_clear();
    for key in ["SystemRoot", "PATH", "TEMP", "TMP"] {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().map_err(|e| WorkerError::Crashed(format!("não iniciou: {e}")))?;
    let mut stdin = child.stdin.take().ok_or_else(|| WorkerError::Crashed("sem stdin".into()))?;
    let stdout = child.stdout.take().ok_or_else(|| WorkerError::Crashed("sem stdout".into()))?;

    // Escrita e leitura em threads próprias: um filho que não consome o pedido ou não termina de
    // responder não pode bloquear o prazo.
    let payload = format!("{request}\n").into_bytes();
    std::thread::spawn(move || {
        let _ = stdin.write_all(&payload);
    });
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.take(MAX_RESPONSE_BYTES + 1).read_to_end(&mut buf);
        let _ = tx.send(buf);
    });

    match rx.recv_timeout(timeout) {
        Ok(buf) => {
            let status = child.wait().map_err(|e| WorkerError::Crashed(format!("wait: {e}")))?;
            if buf.len() as u64 > MAX_RESPONSE_BYTES {
                return Err(WorkerError::Crashed("resposta maior que o limite".into()));
            }
            if !status.success() {
                return Err(WorkerError::Crashed(format!("worker saiu com {status}")));
            }
            Ok(buf)
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait(); // coleta: sem processo zumbi
            Err(WorkerError::Timeout)
        }
    }
}

fn parse_response(raw: &[u8]) -> Result<Result<CompiledShader, String>, WorkerError> {
    let bad = |why: &str| WorkerError::Crashed(format!("resposta inválida: {why}"));
    let v: Value = serde_json::from_slice(raw).map_err(|_| bad("não é JSON"))?;
    if v.get("v").and_then(Value::as_u64) != Some(PROTOCOL_VERSION) {
        return Err(bad("versão"));
    }
    let text = |k: &str| v.get(k).and_then(Value::as_str);
    if v.get("ok").and_then(Value::as_bool) == Some(true) {
        let vs = b64_decode(text("vs").ok_or_else(|| bad("sem vs"))?).ok_or_else(|| bad("vs"))?;
        let ps = b64_decode(text("ps").ok_or_else(|| bad("sem ps"))?).ok_or_else(|| bad("ps"))?;
        if vs.is_empty() || ps.is_empty() {
            return Err(bad("bytecode vazio"));
        }
        return Ok(Ok(CompiledShader { vs, ps, messages: text("messages").unwrap_or_default().to_string() }));
    }
    let error = text("error").unwrap_or("erro sem mensagem").to_string();
    match text("kind") {
        Some("compile") => Ok(Err(error)),
        Some("rejected") => Err(WorkerError::Rejected(error)),
        _ => Err(WorkerError::Crashed(error)),
    }
}

// ---------------------------------------------------------------- fila

/// Vaga de compilação: no máximo `MAX_ACTIVE` simultâneas e `MAX_WAITING` esperando.
struct Slot;

static QUEUE: (Mutex<(usize, usize)>, Condvar) = (Mutex::new((0, 0)), Condvar::new());

impl Slot {
    fn acquire() -> Result<Slot, WorkerError> {
        let (lock, cv) = &QUEUE;
        let mut st = lock.lock().unwrap_or_else(|e| e.into_inner());
        if st.0 >= MAX_ACTIVE {
            if st.1 >= MAX_WAITING {
                return Err(WorkerError::Busy);
            }
            st.1 += 1;
            while st.0 >= MAX_ACTIVE {
                st = cv.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            st.1 -= 1;
        }
        st.0 += 1;
        Ok(Slot)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let (lock, cv) = &QUEUE;
        lock.lock().unwrap_or_else(|e| e.into_inner()).0 -= 1;
        cv.notify_one();
    }
}

// ---------------------------------------------------------------- base64

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let val = |b: u8| B64.iter().position(|&x| x == b).map(|p| p as u32);
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (i, q) in bytes.chunks(4).enumerate() {
        let last = i == bytes.len() / 4 - 1;
        let pad = q.iter().rev().take_while(|&&b| b == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for (j, &b) in q.iter().enumerate() {
            n = n << 6 | if j >= 4 - pad { 0 } else { val(b)? };
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK_SHADER: &str = "float4 ps_main(float4 p : SV_Position) : SV_Target { return float4(1,0,0,1); }";

    fn request(v: Value) -> Vec<u8> {
        format!("{v}\n").into_bytes()
    }

    fn ask(line: &[u8]) -> Value {
        let mut out = Vec::new();
        assert_eq!(serve(&mut std::io::Cursor::new(line.to_vec()), &mut out), 0);
        serde_json::from_slice(&out).unwrap()
    }

    #[test]
    fn base64_roundtrip() {
        for data in [&b""[..], b"a", b"ab", b"abc", b"abcd", &[0, 255, 128, 7, 9]] {
            assert_eq!(b64_decode(&b64_encode(data)).unwrap(), data);
        }
        assert!(b64_decode("abc").is_none());
        assert!(b64_decode("ab=c").is_none());
        assert!(b64_decode("a!cd").is_none());
    }

    #[test]
    fn worker_compiles_valid_and_reports_errors() {
        let ok = ask(&request(json!({"v": 1, "name": "a.hlsl", "source": OK_SHADER})));
        assert_eq!(ok["ok"], true);
        let got = parse_response(ok.to_string().as_bytes()).unwrap().unwrap();
        let direct = compile_uncached(OK_SHADER, "a.hlsl").unwrap();
        assert_eq!((got.vs, got.ps), (direct.vs, direct.ps), "bytecode diferente do compile_uncached");

        let bad = ask(&request(json!({"v": 1, "name": "a.hlsl", "source": "float4 ps_main( {"})));
        assert_eq!((bad["ok"].as_bool(), bad["kind"].as_str()), (Some(false), Some("compile")));
        assert!(parse_response(bad.to_string().as_bytes()).unwrap().is_err());
    }

    #[test]
    fn worker_rejects_limits_version_and_garbage() {
        let big = "a".repeat(MAX_SOURCE_BYTES + 1);
        assert_eq!(ask(&request(json!({"v": 1, "name": "x", "source": big})))["kind"], "rejected");
        assert_eq!(ask(&request(json!({"v": 2, "name": "x", "source": "a"})))["kind"], "rejected");
        assert_eq!(ask(b"nao e json\n")["kind"], "protocol");
        assert_eq!(ask(&request(json!({"v": 1, "name": 3})))["kind"], "protocol");
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend(request(json!({"v": 1, "name": "x", "source": OK_SHADER})));
        assert_eq!(ask(&with_bom)["ok"], true, "BOM UTF-8 deve ser tolerado");
        let huge = vec![b'a'; MAX_REQUEST_BYTES as usize + 10];
        assert_eq!(ask(&huge)["kind"], "rejected");
    }

    #[test]
    fn client_rejects_bad_responses() {
        assert!(matches!(parse_response(b"lixo"), Err(WorkerError::Crashed(_))));
        assert!(matches!(parse_response(br#"{"v":9,"ok":true}"#), Err(WorkerError::Crashed(_))));
        assert!(matches!(parse_response(br#"{"v":1,"ok":true,"vs":"","ps":""}"#), Err(WorkerError::Crashed(_))));
        assert!(matches!(parse_response(br#"{"v":1,"ok":false,"kind":"rejected","error":"x"}"#), Err(WorkerError::Rejected(_))));
    }

    #[cfg(windows)]
    fn ps(script: &str) -> Command {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-Command", script]);
        c
    }

    #[cfg(windows)]
    #[test]
    fn fake_worker_timeout_is_killed_and_next_call_works() {
        let t = std::time::Instant::now();
        let r = compile_with(ps("Start-Sleep 30"), OK_SHADER, "a", Duration::from_millis(700));
        assert_eq!(r.unwrap_err(), WorkerError::Timeout);
        assert!(t.elapsed() < Duration::from_secs(20), "esperou o filho terminar");
        // A fila foi liberada: a próxima compilação passa pelo caminho normal.
        let reply = r#"'{"v":1,"ok":false,"kind":"compile","error":"boom"}'"#;
        let again = compile_with(ps(reply), OK_SHADER, "a", Duration::from_secs(30));
        assert_eq!(again.unwrap().unwrap_err(), "boom");
    }

    #[cfg(windows)]
    #[test]
    fn fake_worker_crash_and_garbage_are_reported() {
        let crash = compile_with(ps("exit 3"), OK_SHADER, "a", Duration::from_secs(30));
        assert!(matches!(crash, Err(WorkerError::Crashed(_))));
        let junk = compile_with(ps("'isto nao e json'"), OK_SHADER, "a", Duration::from_secs(30));
        assert!(matches!(junk, Err(WorkerError::Crashed(_))));
        let missing = compile_with(Command::new("nao_existe_xyz.exe"), OK_SHADER, "a", Duration::from_secs(5));
        assert!(matches!(missing, Err(WorkerError::Crashed(_))));
    }

    #[test]
    fn client_rejects_oversized_source_without_spawning() {
        let big = "a".repeat(MAX_SOURCE_BYTES + 1);
        let r = compile_with(Command::new("nao_existe_xyz.exe"), &big, "a", Duration::from_secs(1));
        assert!(matches!(r, Err(WorkerError::Rejected(_))));
    }
}
