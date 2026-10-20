//! IsNotAEngine — Native Animated Wallpaper Engine for Windows.
//!
//! Ponto de entrada do executável.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod audio;
mod compiler_worker;
mod config;
mod console;
mod control;
mod desktop;
mod engine;
mod import;
mod lab;
mod mcp;
mod media;
mod pause;
mod performance;
mod playlist;
mod render;
mod shadertoy;
mod studio;
mod system;
mod wallpaper;


use config::EngineConfig;
use engine::Engine;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

/// DLLs só do System32: o Windows não procura mais na pasta do app nem no diretório atual (DLL planting).
fn restrict_dll_search() {
    use windows::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories};
    let _ = unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) };
}

fn main() {
    restrict_dll_search();
    // Servidor MCP: o stdout é só do protocolo, então os logs vão para o stderr.
    let early_args: Vec<String> = std::env::args().collect();
    // App de janela: quem chama um utilitário pelo terminal (`--lab`, `--mcp-config`...) usa o console dele.
    if early_args.len() > 1 {
        console::attach_parent();
    }
    // Worker de compilação: sem logging, desktop nem instância única; stdout é só do protocolo.
    if early_args.iter().any(|a| a == "--compile-worker") {
        std::process::exit(compiler_worker::worker_main());
    }
    if early_args.iter().any(|a| a == "--mcp-config") {
        print!("{}", mcp::config_help());
        return;
    }
    if early_args.iter().any(|a| a == "--mcp") {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("IsNotAEngine=warn")))
            .with_writer(std::io::stderr)
            .with_target(false)
            .init();
        std::process::exit(mcp::run());
    }

    // Inicializa o logging estruturado com tracing primeiro para capturar tudo
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("IsNotAEngine=info")),
        )
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    // Panics (inclusive em threads secundárias) vão para o log em vez de sumirem com o processo.
    std::panic::set_hook(Box::new(|info| {
        error!(panic = %info, "Panic no IsNotAEngine");
    }));

    info!(
        "=== IsNotAEngine v{} inicializando ===",
        env!("CARGO_PKG_VERSION")
    );

    // Modo utilitário: `--render-previews <pasta> [id...]` renderiza os shaders do catálogo
    // em BMP (t = 3s e 12s) sem tocar no desktop. Útil para revisar wallpapers.
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--lab") {
        std::process::exit(lab::run(&args[i + 1..]));
    }
    if let Some(i) = args.iter().position(|a| a == "--render-previews") {
        let dir = std::path::PathBuf::from(args.get(i + 1).map(String::as_str).unwrap_or("previews"));
        let only: Vec<&str> = args.iter().skip(i + 2).map(String::as_str).collect();
        std::process::exit(match render_previews(&dir, &only) {
            Ok(()) => 0,
            Err(e) => {
                error!(error = %e, "Falha ao renderizar prévias");
                1
            }
        });
    }

    if args.iter().skip(1).any(|a| {
        let lower = a.to_ascii_lowercase();
        lower == "/p" || lower == "-p" || lower.starts_with("/p:") || lower.starts_with("-p:")
    }) {
        return;
    }
    let screensaver_arg = args.iter().skip(1).any(|a| {
        let lower = a.to_ascii_lowercase();
        lower == "--screensaver" || lower == "/s" || lower == "-s"
    });
    let screensaver_config_arg = args.iter().skip(1).any(|a| {
        let lower = a.to_ascii_lowercase();
        lower == "/c" || lower == "-c" || lower.starts_with("/c:") || lower.starts_with("-c:")
    });

    // 1. Se o processo foi iniciado em um desktop isolado (sandbox), reinicia no desktop interativo 'WinSta0\Default'
    if desktop::discovery::relaunch_on_default_desktop() {
        return;
    }

    // 2. Garante instância única via Mutex nomeado. Se já houver outra instância rodando,
    // apenas envia sinal para abrir o Studio (ou iniciar a proteção de tela) e encerra.
    let _single_instance_mutex = unsafe {
        use windows::Win32::Foundation::*;
        use windows::Win32::System::Threading::*;
        use windows::Win32::UI::WindowsAndMessaging::*;
        use windows::core::w;

        let mutex_name = w!("IsNotAEngine_SingleInstance_Mutex");
        let mutex = CreateMutexW(None, true, mutex_name);
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if screensaver_arg {
                info!("Instância ativa detectada. Solicitando proteção de tela viva...");
                if let Ok(listener) = FindWindowW(w!("IsNotAEngine_SystemListener"), None) {
                    let _ = PostMessageW(
                        Some(listener),
                        WM_COMMAND,
                        WPARAM(crate::desktop::tray::IDM_START_SCREENSAVER),
                        LPARAM(0),
                    );
                }
                return;
            }
            info!("Instância ativa detectada. Enviando comando para abrir o Studio...");
            if let Ok(listener) = FindWindowW(w!("IsNotAEngine_SystemListener"), None) {
                let _ = PostMessageW(
                    Some(listener),
                    WM_COMMAND,
                    WPARAM(crate::desktop::tray::IDM_OPEN_STUDIO),
                    LPARAM(0),
                );
            }
            if let Ok(studio) = FindWindowW(w!("IsNotAEngine_StudioWindowV2"), None) {
                let _ = ShowWindow(studio, SW_SHOWNORMAL);
                let _ = SetForegroundWindow(studio);
            }
            info!("Comando enviado à instância ativa. Encerrando processo duplicado.");
            return;
        }
        mutex
    };

    // Garante desktop interativo ANTES de inicializar COM e quaisquer janelas
    desktop::discovery::ensure_interactive_desktop();

    // Inicializa COM como STA (Single-Threaded Apartment) para WebView2 e UI do Windows
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
    }

    let mut config = EngineConfig::from_args();
    if screensaver_arg {
        config.open_studio = false;
    } else if screensaver_config_arg {
        config.open_studio = true;
    }

    match Engine::new(config) {
        Ok(mut engine) => {
            if screensaver_arg {
                engine.start_screensaver(true);
            }
            if let Err(e) = engine.run() {
                error!(error = %e, "Erro durante a execução do motor");
            }
        }
        Err(e) => {
            error!(error = %e, "Falha crítica na inicialização do motor");
        }
    }

    info!("=== IsNotAEngine encerrado ===");
}

fn render_previews(dir: &std::path::Path, only: &[&str]) -> windows::core::Result<()> {
    use render::offscreen::{OffscreenRenderer, encode_bmp, preview_constants};
    use wallpaper::catalog::{CATALOG, Kind, with_common};

    let _ = std::fs::create_dir_all(dir);
    let mut renderer = OffscreenRenderer::new()?;
    for entry in CATALOG.iter().filter(|e| e.kind == Kind::Shader) {
        if !only.is_empty() && !only.contains(&entry.id) {
            continue;
        }
        let dev = std::path::Path::new("assets/wallpapers/shaders").join(format!("{}.hlsl", entry.id));
        let code = std::fs::read_to_string(&dev).unwrap_or_else(|_| entry.source.to_string());
        let code = with_common(&code);
        for t in [3.0f32, 12.0] {
            let (w, h) = (960, 540);
            let started = std::time::Instant::now();
            let pixels = renderer.render(entry.id, &code, preview_constants(w, h, t, [0.0; 4]))?;
            let out = dir.join(format!("{}_{}.bmp", entry.id, t as u32));
            let _ = std::fs::write(&out, encode_bmp(&pixels, w, h));
            info!(id = entry.id, t, ms = started.elapsed().as_millis() as u64, "Prévia renderizada");
        }
    }
    Ok(())
}
