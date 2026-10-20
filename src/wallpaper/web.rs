//! Wallpapers Web (HTML/CSS/JS/WebGL) hospedados em WebView2 dentro da camada do desktop.
//!
//! Wallpapers do catálogo são servidos direto da memória pelo protocolo `wallpaper://`,
//! então funcionam independentemente do diretório de trabalho. Wallpapers importados são
//! servidos a partir da própria pasta, com bloqueio de travessia para fora dela.
//!
//! A página recebe a ponte `window.engine`:
//! `engine.audio` (`{bass, mid, treble, volume, spectrum[16]}`), `engine.onAudio(cb)`,
//! `engine.media` (música tocando no Windows, ou `null`) e `engine.onMedia(cb)`.

use std::borrow::Cow;
use std::cell::Cell;
use std::num::NonZeroIsize;
use std::path::{Path, PathBuf};
use std::time::Instant;
use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
use tracing::{error, info};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};
use windows::core::{Error, Result};
use wry::http::{Request, Response};
use wry::{WebContext, WebView, WebViewBuilder};

use super::Wallpaper;
use super::catalog::Entry;
use crate::render::RenderSurface;
use crate::render::pipeline::ShaderConstants;

/// Ponte injetada antes de qualquer script da página.
const BRIDGE_JS: &str = r#"
window.engine = {
  audio: { bass: 0, mid: 0, treble: 0, volume: 0, spectrum: new Array(16).fill(0) },
  media: null,
  system: null,                       // métricas do sistema (cpu, cores, mem, disk, net, uptime_secs), atualizadas a cada 1 s
  fps: 60,                            // FPS configurado no Studio; requestAnimationFrame é limitado a ele
  tint: null,                         // cor de destaque do Studio ({r,g,b} 0..255) ou null = automático
  _cbs: [], _mcbs: [], _tcbs: [], _scbs: [],
  onAudio(cb) { this._cbs.push(cb); },
  onMedia(cb) { this._mcbs.push(cb); if (this.media !== null) cb(this.media); },
  onTint(cb) { this._tcbs.push(cb); if (this.tint) cb(this.tint); },
  // Páginas com a meta retint (cor de referência em hexadecimal) têm o matiz girado automaticamente até a cor escolhida.
  _applyRetint() {
    const meta = document.querySelector('meta[name="retint"]');
    const root = document.documentElement;
    if (!meta || !root) return;
    const hue = (r, g, b) => {
      r /= 255; g /= 255; b /= 255;
      const mx = Math.max(r, g, b), d = mx - Math.min(r, g, b);
      if (!d) return 0;
      const h = mx === r ? ((g - b) / d) % 6 : mx === g ? (b - r) / d + 2 : (r - g) / d + 4;
      return (h * 60 + 360) % 360;
    };
    const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(meta.content.trim());
    if (!this.tint || !m) { root.style.filter = ''; return; }
    const ref = hue(parseInt(m[1], 16), parseInt(m[2], 16), parseInt(m[3], 16));
    const deg = Math.round(hue(this.tint.r, this.tint.g, this.tint.b) - ref);
    root.style.filter = deg ? `hue-rotate(${deg}deg)` : '';
  },
  _setTint(t) {
    this.tint = t;
    const run = () => { this._applyRetint(); for (const cb of this._tcbs) { try { cb(t); } catch (e) {} } };
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', run, { once: true }); else run();
  },
  onSystem(cb) { this._scbs.push(cb); if (this.system) cb(this.system); },
  _setSystem(s) {
    this.system = s;
    for (const cb of this._scbs) { try { cb(s); } catch (e) {} }
  },
  _setFps(n) { if (n >= 1 && n <= 240) this.fps = n; },
  _setMedia(m) {
    this.media = m;
    for (const cb of this._mcbs) { try { cb(m); } catch (e) {} }
  },
  _push(b, m, t, v, s) {
    const a = this.audio; a.bass = b; a.mid = m; a.treble = t; a.volume = v; a.spectrum = s;
    for (const cb of this._cbs) { try { cb(a); } catch (e) {} }
  }
};
// Telas de 120/144 Hz fariam as páginas rodar acima do FPS do motor: limita requestAnimationFrame ao FPS do Studio.
(function () {
  const raf = window.requestAnimationFrame.bind(window);
  let nextId = 1, queue = [], scheduled = false, gate = 0;
  function pump(t) {
    scheduled = false;
    if (t - gate < 1000 / (window.engine.fps || 60) - 2) { scheduled = true; raf(pump); return; }
    gate = t;
    const run = queue; queue = [];
    for (const it of run) { if (it.cb) { try { it.cb(t); } catch (e) { setTimeout(() => { throw e; }); } } }
    if (queue.length && !scheduled) { scheduled = true; raf(pump); }
  }
  window.requestAnimationFrame = cb => {
    const it = { id: nextId++, cb }; queue.push(it);
    if (!scheduled) { scheduled = true; raf(pump); }
    return it.id;
  };
  window.cancelAnimationFrame = id => { for (const it of queue) if (it.id === id) it.cb = null; };
})();
document.addEventListener('contextmenu', e => e.preventDefault());
"#;

const AUDIO_INTERVAL_MS: u128 = 33;

pub enum WebSource {
    Builtin(&'static Entry),
    Dir { root: PathBuf, entry: String },
}

/// Origem (`esquema://host[:porta]`, minúscula) de uma URL; `None` se não for absoluta ou tiver credenciais.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty() && !host.contains('@')).then(|| format!("{}://{}", scheme.to_ascii_lowercase(), host.to_ascii_lowercase()))
}

/// Navegação permitida: só dentro do protocolo `wallpaper` (que o WebView2 expõe como
/// `http(s)://wallpaper.localhost`). Todo o resto (sites, `file:`, `javascript:`, esquemas externos)
/// é bloqueado.
fn navigation_allowed(url: &str) -> bool {
    if url == "about:blank" {
        return true;
    }
    let Some(origin) = origin_of(url) else { return false };
    matches!(origin.as_str(), "wallpaper://localhost" | "http://wallpaper.localhost" | "https://wallpaper.localhost")
}

struct WindowWrapper(HWND);

impl HasWindowHandle for WindowWrapper {
    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        let handle = Win32WindowHandle::new(NonZeroIsize::new(self.0.0 as isize).unwrap());
        unsafe { Ok(WindowHandle::borrow_raw(RawWindowHandle::Win32(handle))) }
    }
}

pub struct WebWallpaper {
    name: String,
    /// Página de fábrica (do catálogo). Páginas importadas são código de terceiros: recebem o espectro de áudio,
    /// mas **não** a música em reprodução nem as métricas do sistema.
    trusted: bool,
    webview: WebView,
    _context: WebContext,
    paused: Cell<bool>,
    last_audio: Cell<Option<Instant>>,
    created: Instant,
    media_version: Cell<u64>,
    last_media_push: Cell<Option<Instant>>,
    system_version: Cell<u64>,
    last_system_push: Cell<Option<Instant>>,
    last_fps: Cell<u32>,
    last_fps_push: Cell<Option<Instant>>,
    last_tint: Cell<Option<[u8; 4]>>,
    last_tint_push: Cell<Option<Instant>>,
}

unsafe impl Send for WebWallpaper {}

pub fn mime_type(path: &str) -> &'static str {
    let ext = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "glsl" | "frag" | "vert" | "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Wallpapers locais não acessam a rede: scripts, estilos e mídia só da própria origem
/// (`wallpaper://`) ou embutidos/inline; `fetch`/XHR/WebSocket bloqueados.
pub(crate) const SANDBOX_CSP: &str = "default-src 'self' data: blob:; script-src 'self' 'unsafe-inline' 'unsafe-eval';     style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' data: blob:;     font-src 'self' data:; connect-src 'none'; frame-src 'none'; object-src 'none'";

fn respond(status: u16, mime: &str, body: Cow<'static, [u8]>) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header("Content-Type", mime)
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .header("Content-Security-Policy", SANDBOX_CSP)
        .body(body)
        .unwrap()
}

fn not_found() -> Response<Cow<'static, [u8]>> {
    respond(404, "text/plain", Cow::Borrowed(b"404"))
}

/// Resolve um arquivo dentro de `root` sem permitir escapar da pasta.
fn serve_from_dir(root: &Path, rel: &str) -> Response<Cow<'static, [u8]>> {
    let Ok(root) = root.canonicalize() else { return not_found() };
    let Ok(file) = root.join(rel).canonicalize() else { return not_found() };
    if !file.starts_with(&root) || !file.is_file() {
        return not_found();
    }
    match std::fs::read(&file) {
        Ok(bytes) => respond(200, mime_type(rel), Cow::Owned(bytes)),
        Err(_) => not_found(),
    }
}

const ERROR_BUSY_HRESULT: i32 = 0x800700AAu32 as i32;

fn is_busy(e: &wry::Error) -> bool {
    matches!(e, wry::Error::WebView2Error(webview2_com::Error::WindowsError(w)) if w.code().0 == ERROR_BUSY_HRESULT)
}

/// Cria uma WebView repetindo enquanto a pasta de dados estiver em uso (`ERROR_BUSY`).
///
/// Acontece ao reiniciar o motor: os processos `msedgewebview2` da execução anterior
/// ainda seguram a pasta por alguns segundos. Se não liberar a tempo, usa uma pasta
/// exclusiva deste processo para não ficar sem wallpaper/Studio.
pub fn with_busy_retry<T>(
    mut build: impl FnMut(PathBuf) -> std::result::Result<T, wry::Error>,
    data_dir: &Path,
) -> std::result::Result<T, wry::Error> {
    const ATTEMPTS: u32 = 12;
    for attempt in 0..ATTEMPTS {
        match build(data_dir.to_path_buf()) {
            Err(e) if is_busy(&e) && attempt + 1 < ATTEMPTS => {
                tracing::warn!(attempt, "Pasta do WebView2 em uso; tentando novamente");
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            Err(e) if is_busy(&e) => break,
            other => return other,
        }
    }
    let fallback = data_dir.with_file_name(format!(
        "{}_{}",
        data_dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        std::process::id()
    ));
    tracing::warn!(dir = %fallback.display(), "Usando pasta alternativa para o WebView2");
    build(fallback)
}

impl WebWallpaper {
    pub fn new(hwnd: HWND, source: WebSource) -> Result<Self> {
        let (name, url) = match &source {
            WebSource::Builtin(e) => (e.id.to_string(), "wallpaper://localhost/index.html".to_string()),
            WebSource::Dir { root, entry } => (
                Path::new(entry).file_stem().map(|s| s.to_string_lossy().to_string())
                    .filter(|s| s != "index")
                    .or_else(|| root.file_name().map(|s| s.to_string_lossy().to_string()))
                    .unwrap_or_else(|| "web".into()),
                format!("wallpaper://localhost/{}", entry.replace('\\', "/")),
            ),
        };
        info!(name = %name, url = %url, "Criando WebWallpaper");

        let base_dir = crate::config::EngineConfig::config_path()
            .parent()
            .unwrap_or(Path::new("."))
            .join("webview_wallpaper_data");

        let builtin = match &source {
            WebSource::Builtin(e) => Some(e.source),
            _ => None,
        };
        let root = match &source {
            WebSource::Dir { root, .. } => Some(root.clone()),
            _ => None,
        };

        let (webview, context) = with_busy_retry(|data_dir| {
            let _ = std::fs::create_dir_all(&data_dir);
            let mut context = WebContext::new(Some(data_dir));
            let root = root.clone();
            let webview = WebViewBuilder::new_with_web_context(&mut context)
                .with_background_color((0, 0, 0, 255))
                .with_devtools(false)
                .with_hotkeys_zoom(false)
                .with_focused(false)
                .with_initialization_script(BRIDGE_JS)
                .with_navigation_handler(move |url| {
                    let ok = navigation_allowed(&url);
                    if !ok {
                        tracing::warn!(%url, "Navegação do wallpaper bloqueada");
                    }
                    ok
                })
                .with_new_window_req_handler(|url, _| {
                    tracing::warn!(%url, "Nova janela do wallpaper bloqueada");
                    wry::NewWindowResponse::Deny
                })
                .with_permission_handler(|kind| {
                    tracing::warn!(?kind, "Permissão do navegador negada ao wallpaper");
                    wry::PermissionResponse::Deny
                })
                .with_download_started_handler(|url, _| {
                    tracing::warn!(%url, "Download do wallpaper bloqueado");
                    false
                })
                .with_custom_protocol("wallpaper".into(), move |_id, req: Request<Vec<u8>>| {
                    let rel = percent_decode(req.uri().path().trim_start_matches('/'));
                    let rel = if rel.is_empty() { "index.html".to_string() } else { rel };
                    if let Some(html) = builtin {
                        return if rel == "index.html" {
                            respond(200, "text/html; charset=utf-8", Cow::Borrowed(html.as_bytes()))
                        } else {
                            not_found()
                        };
                    }
                    match &root {
                        Some(root) => serve_from_dir(root, &rel),
                        None => not_found(),
                    }
                })
                .with_url(&url)
                .build(&WindowWrapper(hwnd))?;
            Ok((webview, context))
        }, &base_dir)
        .map_err(|e| {
            error!(error = %e, url = %url, "Falha ao criar WebView2 do wallpaper");
            Error::new(E_FAIL, e.to_string())
        })?;

        Ok(Self {
            name,
            trusted: matches!(source, WebSource::Builtin(_)),
            webview,
            _context: context,
            paused: Cell::new(false),
            last_audio: Cell::new(None),
            created: Instant::now(),
            media_version: Cell::new(u64::MAX),
            last_media_push: Cell::new(None),
            system_version: Cell::new(u64::MAX),
            last_system_push: Cell::new(None),
            last_fps: Cell::new(0),
            last_fps_push: Cell::new(None),
            last_tint: Cell::new(None),
            last_tint_push: Cell::new(None),
        })
    }

    /// Captura a página como PNG (para espelhar no wallpaper do Windows). Bombeia mensagens
    /// enquanto espera; `None` se a WebView estiver oculta/pausada ou a captura falhar.
    pub fn capture_png(&self) -> Option<Vec<u8>> {
        use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG;
        use windows::Win32::System::Com::{IStream, STREAM_SEEK_SET, STATSTG, STATFLAG_NONAME};
        use windows::Win32::UI::Shell::SHCreateMemStream;
        use wry::WebViewExtWindows;

        if self.paused.get() {
            return None;
        }
        let core = unsafe { self.webview.controller().CoreWebView2().ok()? };
        let stream: IStream = unsafe { SHCreateMemStream(None)? };
        let (tx, rx) = std::sync::mpsc::channel();
        let handler = webview2_com::CapturePreviewCompletedHandler::create(Box::new(move |r| {
            let _ = tx.send(r);
            Ok(())
        }));
        unsafe { core.CapturePreview(COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, &stream, &handler).ok()? };
        webview2_com::wait_with_pump(rx).ok()?.ok()?;
        unsafe {
            let mut stat = STATSTG::default();
            stream.Stat(&mut stat, STATFLAG_NONAME).ok()?;
            stream.Seek(0, STREAM_SEEK_SET, None).ok()?;
            let mut buf = vec![0u8; stat.cbSize as usize];
            let mut read = 0u32;
            stream.Read(buf.as_mut_ptr() as *mut _, buf.len() as u32, Some(&mut read)).ok().ok()?;
            buf.truncate(read as usize);
            (!buf.is_empty()).then_some(buf)
        }
    }

    /// Envia a música atual quando muda. Nos primeiros segundos reenvia periodicamente,
    /// pois a página pode ainda não ter carregado quando o primeiro envio acontece.
    pub fn update_media(&self, version: u64, json: &str) {
        if !self.trusted {
            return;
        }
        let now = Instant::now();
        let warming = now.duration_since(self.created).as_secs() < 8
            && self.last_media_push.get().is_none_or(|t| now.duration_since(t).as_millis() > 1000);
        if self.media_version.get() == version && !warming {
            return;
        }
        self.media_version.set(version);
        self.last_media_push.set(Some(now));
        let _ = self.webview.evaluate_script(&format!("window.engine&&engine._setMedia({json})"));
    }

    /// Envia as métricas do sistema quando a versão muda (1 Hz). Nos primeiros segundos reenvia a cada 1 s,
    /// pois a página pode ainda não ter carregado quando o primeiro envio acontece.
    pub fn update_system(&self, version: u64, json: &str) {
        if !self.trusted {
            return;
        }
        if version == 0 || json.is_empty() {
            return;
        }
        let now = Instant::now();
        let warming = now.duration_since(self.created).as_secs() < 8
            && self.last_system_push.get().is_none_or(|t| now.duration_since(t).as_millis() > 1000);
        if self.system_version.get() == version && !warming {
            return;
        }
        if self.system_version.get() == u64::MAX {
            tracing::info!(wallpaper = %self.name, "Métricas do sistema enviadas ao wallpaper (primeira entrega)");
        }
        self.system_version.set(version);
        self.last_system_push.set(Some(now));
        let _ = self.webview.evaluate_script(&format!("window.engine&&engine._setSystem({json})"));
    }

    /// Envia o FPS configurado quando muda (e reenvia a cada 1 s nos primeiros segundos, até a página carregar).
    pub fn update_fps(&self, fps: u32) {
        let now = Instant::now();
        let warming = now.duration_since(self.created).as_secs() < 8
            && self.last_fps_push.get().is_none_or(|t| now.duration_since(t).as_millis() > 1000);
        if self.last_fps.get() == fps && !warming {
            return;
        }
        self.last_fps.set(fps);
        self.last_fps_push.set(Some(now));
        let _ = self.webview.evaluate_script(&format!("window.engine&&engine._setFps({fps})"));
    }

    /// Envia a cor de destaque do Studio (`[r,g,b,flag]`, flag 0 = automático) quando muda. Nos primeiros
    /// segundos reenvia a cada 1 s, pois a página pode ainda não ter carregado.
    pub fn update_tint(&self, tint: [f32; 4]) {
        let q = tint.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
        let now = Instant::now();
        let warming = now.duration_since(self.created).as_secs() < 8
            && self.last_tint_push.get().is_none_or(|t| now.duration_since(t).as_millis() > 1000);
        let first_auto = self.last_tint.get().is_none() && q[3] == 0;
        if self.last_tint.get() == Some(q) && !warming {
            return;
        }
        if first_auto && !warming {
            self.last_tint.set(Some(q));
            return;
        }
        self.last_tint.set(Some(q));
        self.last_tint_push.set(Some(now));
        let arg = if q[3] > 0 { format!("{{r:{},g:{},b:{}}}", q[0], q[1], q[2]) } else { "null".to_string() };
        let _ = self.webview.evaluate_script(&format!("window.engine&&engine._setTint({arg})"));
    }

    /// Envia o estado de áudio para a página (limitado a ~30 Hz e só quando visível).
    pub fn update_audio(&self, bass: f32, mid: f32, treble: f32, volume: f32, spectrum: &[f32; 16]) {
        if self.paused.get() {
            return;
        }
        let now = Instant::now();
        if let Some(prev) = self.last_audio.get()
            && now.duration_since(prev).as_millis() < AUDIO_INTERVAL_MS
        {
            return;
        }
        self.last_audio.set(Some(now));

        let spec: Vec<String> = spectrum.iter().map(|v| format!("{v:.3}")).collect();
        let script = format!(
            "window.engine&&engine._push({bass:.3},{mid:.3},{treble:.3},{volume:.3},[{}])",
            spec.join(",")
        );
        let _ = self.webview.evaluate_script(&script);
    }
}

impl Wallpaper for WebWallpaper {
    fn name(&self) -> &str {
        &self.name
    }

    fn render(&self, _: &ID3D11DeviceContext, _: &RenderSurface, _: ShaderConstants) -> Result<()> {
        // O WebView2 desenha sozinho na própria janela.
        Ok(())
    }

    fn check_reload(&mut self, _device: &ID3D11Device) -> Result<bool> {
        Ok(false)
    }

    fn set_paused(&self, paused: bool) {
        if self.paused.replace(paused) != paused {
            // Ocultar a WebView faz o WebView2 suspender timers e requestAnimationFrame.
            let _ = self.webview.set_visible(!paused);
        }
    }

    fn is_web(&self) -> bool {
        true
    }

    fn as_web(&self) -> Option<&WebWallpaper> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_handles_utf8_and_edges() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("caf%C3%A9.html"), "café.html");
        assert_eq!(percent_decode("bad%2"), "bad%2");
        assert_eq!(percent_decode("%"), "%");
    }

    #[test]
    fn serve_from_dir_blocks_traversal() {
        let dir = std::env::temp_dir().join(format!("inae_web_{}", std::process::id()));
        let inner = dir.join("wp");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("index.html"), "ok").unwrap();
        std::fs::write(dir.join("secret.txt"), "no").unwrap();
        assert_eq!(serve_from_dir(&inner, "index.html").status(), 200);
        assert_eq!(serve_from_dir(&inner, "../secret.txt").status(), 404);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn navigation_stays_inside_the_wallpaper_origin() {
        for ok in ["wallpaper://localhost/index.html", "http://wallpaper.localhost/a.html", "https://wallpaper.localhost/", "about:blank"] {
            assert!(navigation_allowed(ok), "{ok}");
        }
        for bad in ["https://exemplo.com/", "file:///C:/segredo.txt", "javascript:alert(1)", "ms-settings:", "http://wallpaper.localhost.evil.com/", "http://wallpaper.localhost@evil.com/", "data:text/html,x"] {
            assert!(!navigation_allowed(bad), "{bad}");
        }
        assert_eq!(origin_of("https://Site.com/pagina?x=1").as_deref(), Some("https://site.com"));
        assert_eq!(origin_of("https://u@site.com/"), None);
    }
}
