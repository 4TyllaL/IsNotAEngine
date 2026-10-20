//! Transporte Win32 do canal de controle: servidor de named pipe.
//!
//! Todo o `unsafe` do canal fica neste arquivo. Modelo: uma thread aceita conexões
//! (bloqueada em `ConnectNamedPipe`, 0% CPU quando ociosa) e cada cliente ganha uma thread
//! própria, no máximo `MAX_CLIENTS`. O pipe só aceita o mesmo usuário (e SYSTEM) e recusa
//! clientes remotos. Comandos viram `EngineCommand` e seguem pelo canal do motor.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};

use tracing::{debug, info, warn};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::core::{PCWSTR, w};

use super::{MAX_LINE, Request, StateSnapshot, event_line, response_err, response_ok};
use crate::engine::EngineCommand;

pub const PIPE_NAME: &str = r"\\.\pipe\IsNotAEngine.Control.v1";

const MAX_CLIENTS: usize = 8;
const BUF_SIZE: u32 = 4096;
/// Só o dono do processo e SYSTEM; sem leitura para Everyone/anônimo (padrão dos pipes).
const SDDL: PCWSTR = w!("D:P(A;;GA;;;OW)(A;;GA;;;SY)");

struct Shared {
    latest_json: String,
    subscribers: Vec<SyncSender<String>>,
}

/// Dono do servidor; `publish` é chamado pelo motor quando o estado muda.
pub struct ControlServer {
    shared: Arc<Mutex<Shared>>,
}

impl ControlServer {
    /// Inicia o servidor. Falha (só registrada no log) não impede o motor de rodar.
    pub fn start(cmd_tx: Sender<EngineCommand>, initial: &StateSnapshot) -> Option<Self> {
        let shared = Arc::new(Mutex::new(Shared {
            latest_json: serde_json::to_string(initial).unwrap_or_else(|_| "{}".into()),
            subscribers: Vec::new(),
        }));

        // Cria a primeira instância aqui para detectar "pipe já existe" antes de seguir.
        let first = match create_instance(true) {
            Ok(h) => h,
            Err(e) => {
                warn!(error = %e, pipe = PIPE_NAME, "Canal de controle indisponível; o motor segue sem ele");
                return None;
            }
        };

        let acceptor_shared = shared.clone();
        let first = SendHandle(first.0 as isize);
        let spawned = std::thread::Builder::new()
            .name("ControlPipe".into())
            .spawn(move || {
                let first = first;
                accept_loop(first, cmd_tx, acceptor_shared);
            });
        if let Err(e) = spawned {
            warn!(error = %e, "Não foi possível iniciar a thread do canal de controle");
            return None;
        }
        info!(pipe = PIPE_NAME, "Canal de controle ativo");
        Some(Self { shared })
    }

    /// Publica o novo estado: atualiza o cache e empurra o evento aos assinantes.
    pub fn publish(&self, state: &StateSnapshot) {
        let Ok(json) = serde_json::to_string(state) else {
            return;
        };
        let Ok(mut shared) = self.shared.lock() else {
            return;
        };
        let line = event_line(&json);
        // `try_send`: assinante lento ou desconectado é descartado, nunca trava o motor.
        shared
            .subscribers
            .retain(|tx| tx.try_send(line.clone()).is_ok());
        shared.latest_json = json;
    }
}

/// `HANDLE` cru não é `Send`; o valor só é usado por uma thread por vez.
struct SendHandle(isize);
impl SendHandle {
    fn handle(&self) -> HANDLE {
        HANDLE(self.0 as *mut _)
    }
}
unsafe impl Send for SendHandle {}

/// Cria uma instância do pipe. `first` exige que seja a primeira (evita pipe falso de outro processo).
fn create_instance(first: bool) -> windows::core::Result<HANDLE> {
    unsafe {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(SDDL, SDDL_REVISION_1, &mut sd, None)?;
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };
        let mut open_mode = PIPE_ACCESS_DUPLEX;
        if first {
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        let name: Vec<u16> = PIPE_NAME.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            BUF_SIZE,
            BUF_SIZE,
            0,
            Some(&sa),
        );
        // O descritor foi copiado para o objeto do pipe; libera o da conversão.
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        if handle == INVALID_HANDLE_VALUE {
            return Err(windows::core::Error::from_thread());
        }
        Ok(handle)
    }
}

fn accept_loop(first: SendHandle, cmd_tx: Sender<EngineCommand>, shared: Arc<Mutex<Shared>>) {
    let active = Arc::new(AtomicUsize::new(0));
    let mut next = Some(first.handle());
    loop {
        let pipe = match next.take() {
            Some(h) => h,
            None => match create_instance(false) {
                Ok(h) => h,
                Err(e) => {
                    warn!(error = %e, "Falha ao criar instância do pipe; encerrando o canal de controle");
                    return;
                }
            },
        };

        // Bloqueia até um cliente conectar. Já conectado entre o Create e o Connect não é erro.
        let connected = unsafe {
            match ConnectNamedPipe(pipe, None) {
                Ok(()) => true,
                Err(e) => e.code() == ERROR_PIPE_CONNECTED.to_hresult(),
            }
        };
        if !connected {
            unsafe {
                let _ = CloseHandle(pipe);
            }
            continue;
        }

        if active.load(Ordering::SeqCst) >= MAX_CLIENTS {
            debug!("Canal de controle: limite de clientes atingido; conexão recusada");
            close_pipe(pipe);
            continue;
        }
        active.fetch_add(1, Ordering::SeqCst);

        let client = SendHandle(pipe.0 as isize);
        let (tx, shared, active_c) = (cmd_tx.clone(), shared.clone(), active.clone());
        let spawned = std::thread::Builder::new()
            .name("ControlClient".into())
            .spawn(move || {
                serve_client(client.handle(), &tx, &shared);
                close_pipe(client.handle());
                active_c.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            close_pipe(pipe);
            active.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

fn close_pipe(pipe: HANDLE) {
    unsafe {
        let _ = FlushFileBuffers(pipe);
        let _ = DisconnectNamedPipe(pipe);
        let _ = CloseHandle(pipe);
    }
}

fn serve_client(pipe: HANDLE, cmd_tx: &Sender<EngineCommand>, shared: &Arc<Mutex<Shared>>) {
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; BUF_SIZE as usize];
    loop {
        // Processa as linhas completas já recebidas.
        while let Some(pos) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=pos).collect();
            let text = String::from_utf8_lossy(&line);
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            match handle_line(text, cmd_tx, shared) {
                Reply::Line(out) => {
                    if write_all(pipe, out.as_bytes()).is_err() {
                        return;
                    }
                }
                Reply::Subscribe(first_line, rx) => {
                    if write_all(pipe, first_line.as_bytes()).is_ok() {
                        stream_events(pipe, &rx);
                    }
                    return;
                }
            }
        }

        if pending.len() > MAX_LINE {
            let _ = write_all(pipe, response_err("linha longa demais").as_bytes());
            return;
        }

        let mut read = 0u32;
        let ok = unsafe { ReadFile(pipe, Some(&mut buf), Some(&mut read), None) };
        if ok.is_err() || read == 0 {
            return; // cliente desconectou
        }
        pending.extend_from_slice(&buf[..read as usize]);
    }
}

enum Reply {
    Line(String),
    Subscribe(String, Receiver<String>),
}

fn handle_line(text: &str, cmd_tx: &Sender<EngineCommand>, shared: &Arc<Mutex<Shared>>) -> Reply {
    let request: Request = match serde_json::from_str(text) {
        Ok(r) => r,
        Err(e) => return Reply::Line(response_err(&format!("requisição inválida: {e}"))),
    };

    if request == Request::ListWallpapers {
        let dir = crate::config::EngineConfig::config_path()
            .parent()
            .map(|p| p.join("wallpapers"))
            .unwrap_or_default();
        return Reply::Line(super::response_wallpapers(&super::list_wallpapers(&dir)));
    }

    let Ok(mut guard) = shared.lock() else {
        return Reply::Line(response_err("estado indisponível"));
    };

    if request == Request::Subscribe {
        let (tx, rx) = sync_channel(16);
        guard.subscribers.push(tx);
        return Reply::Subscribe(response_ok(&guard.latest_json), rx);
    }

    let reply = response_ok(&guard.latest_json);
    drop(guard);
    match request.into_command() {
        Ok(Some(cmd)) => {
            if cmd_tx.send(cmd).is_err() {
                return Reply::Line(response_err("motor encerrando"));
            }
        }
        Ok(None) => {}
        Err(reason) => return Reply::Line(response_err(&reason)),
    }
    Reply::Line(reply)
}

/// Escreve eventos até o cliente sair (falha de escrita) ou o servidor descartá-lo.
fn stream_events(pipe: HANDLE, rx: &Receiver<String>) {
    while let Ok(line) = rx.recv() {
        if write_all(pipe, line.as_bytes()).is_err() {
            return;
        }
    }
}

fn write_all(pipe: HANDLE, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        let mut written = 0u32;
        unsafe { WriteFile(pipe, Some(data), Some(&mut written), None) }
            .map_err(|e| io::Error::other(e.to_string()))?;
        if written == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        data = &data[written as usize..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs::OpenOptions;
    use std::io::{BufRead, BufReader, Write};

    fn snapshot(paused: bool) -> StateSnapshot {
        let mut w = HashMap::new();
        w.insert(0, "matrix".to_string());
        StateSnapshot::new(
            paused,
            false,
            super::super::Settings {
                fps: 60,
                speed: 1.0,
                pause_covered: true,
                pause_battery: false,
                mouse: true,
                parallax: true,
                playlist_enabled: false,
                playlist_minutes: 10,
                playlist_shuffle: false,
            },
            &w,
        )
    }

    /// Ignorado por padrão: usa o nome real do pipe, que colide com uma engine em execução.
    /// Rode com `cargo test control::pipe -- --ignored` com a engine fechada.
    #[test]
    #[ignore]
    fn pipe_roundtrip_commands_and_events() {
        let (tx, rx) = std::sync::mpsc::channel();
        let server =
            ControlServer::start(tx, &snapshot(false)).expect("pipe indisponível (engine aberta?)");

        let open = || {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(PIPE_NAME)
                .unwrap()
        };

        // Estado + comando em uma conexão.
        let mut c = open();
        c.write_all(b"{\"cmd\":\"state\"}\n").unwrap();
        let mut r = BufReader::new(c.try_clone().unwrap());
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        assert!(
            line.starts_with("{\"ok\":true,\"state\":{\"v\":1,\"paused\":false"),
            "{line}"
        );

        c.write_all(b"{\"cmd\":\"pause\"}\n{\"cmd\":\"bogus\"}\n")
            .unwrap();
        line.clear();
        r.read_line(&mut line).unwrap();
        assert!(line.starts_with("{\"ok\":true"));
        line.clear();
        r.read_line(&mut line).unwrap();
        assert!(line.starts_with("{\"ok\":false"), "{line}");
        assert!(matches!(
            rx.recv_timeout(std::time::Duration::from_secs(2)),
            Ok(EngineCommand::SetPaused(true))
        ));

        // Assinatura: resposta imediata e depois um evento por publicação.
        let mut s = open();
        s.write_all(b"{\"cmd\":\"subscribe\"}\n").unwrap();
        let mut sr = BufReader::new(s.try_clone().unwrap());
        line.clear();
        sr.read_line(&mut line).unwrap();
        assert!(line.starts_with("{\"ok\":true"));
        server.publish(&snapshot(true));
        line.clear();
        sr.read_line(&mut line).unwrap();
        assert!(line.starts_with("{\"event\":\"state\""), "{line}");
        assert!(line.contains("\"paused\":true"), "{line}");
    }
}
