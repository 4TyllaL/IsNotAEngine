//! "Tocando agora": lê a sessão de mídia ativa do Windows (Spotify, navegador, players)
//! via `GlobalSystemMediaTransportControlsSessionManager` numa thread própria.
//!
//! O resultado é um JSON pronto para os wallpapers web (`engine.media`), com versão
//! incrementada a cada mudança para o motor só reenviar quando algo mudar.

use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{debug, info};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::Storage::Streams::DataReader;
use windows::core::Result;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct MediaInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub app: String,
    pub playing: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// Capa como data URI (vazio se o player não fornecer).
    pub cover: String,
}

#[derive(Clone, Default)]
pub struct MediaWatcher {
    state: Arc<Mutex<(u64, String)>>,
    /// Modo desempenho: a thread não consulta o Windows enquanto estiver `true`.
    sleeping: Arc<std::sync::atomic::AtomicBool>,
}

impl MediaWatcher {
    pub fn start() -> Self {
        let watcher = Self::default();
        let state = watcher.state.clone();
        let sleeping = watcher.sleeping.clone();
        let _ = std::thread::Builder::new().name("MediaWatcher".into()).spawn(move || {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
            }
            let manager = match SessionManager::RequestAsync().and_then(|op| op.join()) {
                Ok(m) => m,
                Err(e) => {
                    info!(error = %e, "Sessões de mídia do Windows indisponíveis; 'Tocando agora' desativado");
                    return;
                }
            };
            let mut last: Option<MediaInfo> = None;
            let mut cover_key = String::new();
            let mut cover = String::new();
            loop {
                if sleeping.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(1000));
                    continue;
                }
                let info = read_session(&manager, &mut cover_key, &mut cover).unwrap_or_else(|e| {
                    debug!(error = %e, "Falha ao ler sessão de mídia");
                    None
                });
                // Posição muda a cada segundo; só republica quando algo além dela mudar
                // ou quando a posição saltar (seek), para não reenviar a capa sem motivo.
                let changed = match (&last, &info) {
                    (None, None) => false,
                    (Some(a), Some(b)) => {
                        let mut a2 = a.clone();
                        a2.position_ms = b.position_ms;
                        a2 != *b || b.position_ms.abs_diff(a.position_ms) > 2500
                    }
                    _ => true,
                };
                if changed {
                    let json = serde_json::to_string(&info).unwrap_or_else(|_| "null".into());
                    if let Ok(mut s) = state.lock() {
                        s.0 += 1;
                        s.1 = json;
                    }
                    last = info;
                } else if let (Some(l), Some(i)) = (&mut last, &info) {
                    l.position_ms = i.position_ms;
                }
                std::thread::sleep(Duration::from_millis(1000));
            }
        });
        watcher
    }

    pub fn set_sleeping(&self, sleeping: bool) {
        self.sleeping.store(sleeping, std::sync::atomic::Ordering::Relaxed);
    }

    /// Versão atual e JSON (`null` quando nada está tocando).
    pub fn snapshot(&self) -> (u64, String) {
        self.state.lock().map(|s| s.clone()).unwrap_or((0, "null".into()))
    }
}

fn read_session(manager: &SessionManager, cover_key: &mut String, cover: &mut String) -> Result<Option<MediaInfo>> {
    let Ok(session) = manager.GetCurrentSession() else { return Ok(None) };
    let props = session.TryGetMediaPropertiesAsync()?.join()?;
    let title = props.Title()?.to_string();
    if title.is_empty() {
        return Ok(None);
    }
    let artist = props.Artist()?.to_string();
    let album = props.AlbumTitle().map(|s| s.to_string()).unwrap_or_default();

    let key = format!("{title}\u{1}{artist}\u{1}{album}");
    if *cover_key != key {
        *cover_key = key;
        *cover = props
            .Thumbnail()
            .ok()
            .and_then(|r| read_thumbnail(&r).ok())
            .unwrap_or_default();
    }

    let playing = session.GetPlaybackInfo()?.PlaybackStatus()? == PlaybackStatus::Playing;
    let (position_ms, duration_ms) = session
        .GetTimelineProperties()
        .and_then(|t| Ok(((t.Position()?.Duration / 10_000).max(0) as u64, (t.EndTime()?.Duration / 10_000).max(0) as u64)))
        .unwrap_or((0, 0));
    let app = session.SourceAppUserModelId().map(|s| friendly_app(&s.to_string())).unwrap_or_default();

    Ok(Some(MediaInfo { title, artist, album, app, playing, position_ms, duration_ms, cover: cover.clone() }))
}

fn read_thumbnail(reference: &windows::Storage::Streams::IRandomAccessStreamReference) -> Result<String> {
    let stream = reference.OpenReadAsync()?.join()?;
    let size = stream.Size()? as u32;
    if size == 0 || size > 4 * 1024 * 1024 {
        return Ok(String::new());
    }
    let mime = stream.ContentType().map(|s| s.to_string()).unwrap_or_default();
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
    reader.LoadAsync(size)?.join()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes)?;
    let mime = if mime.starts_with("image/") { mime } else { "image/jpeg".into() };
    Ok(format!("data:{mime};base64,{}", base64(&bytes)))
}

/// "Spotify.exe" / "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify" → "Spotify".
fn friendly_app(id: &str) -> String {
    let last = id.rsplit(['!', '\\']).next().unwrap_or(id);
    let name = last.trim_end_matches(".exe").trim_end_matches(".EXE");
    let name = name.rsplit('.').next().unwrap_or(name);
    match name.to_ascii_lowercase().as_str() {
        "msedge" => "Edge".into(),
        "chrome" => "Chrome".into(),
        "firefox" => "Firefox".into(),
        _ => name.to_string(),
    }
}

pub(crate) fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    /// Lê a sessão real do sistema: `cargo test media -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn reads_live_session() {
        let w = MediaWatcher::start();
        std::thread::sleep(Duration::from_secs(4));
        let (v, json) = w.snapshot();
        println!("versão {v}: {}", &json[..json.len().min(400)]);
    }

    #[test]
    fn friendly_app_names() {
        assert_eq!(friendly_app("Spotify.exe"), "Spotify");
        assert_eq!(friendly_app("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"), "Spotify");
        assert_eq!(friendly_app("MSEdge"), "Edge");
    }
}
