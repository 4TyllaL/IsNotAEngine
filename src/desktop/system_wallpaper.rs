//! Espelha o wallpaper animado no wallpaper do Windows com um quadro estático.
//!
//! A engine desenha numa janela filha da camada do desktop; ferramentas como o Recorte
//! (Snipping Tool) mostram por alguns segundos o wallpaper "de verdade" do sistema. Gravar
//! um snapshot do quadro atual ali faz essa imagem coincidir com o que o usuário vê.
//!
//! O wallpaper original do usuário é guardado em `original_wallpapers.json` (só na primeira
//! vez, sobrevive a falhas) e devolvido ao encerrar — a menos que o usuário tenha trocado
//! o wallpaper por conta própria nesse meio-tempo.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};
use windows::Win32::Foundation::RECT;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::{DESKTOP_WALLPAPER_POSITION, DWPOS_FILL, DesktopWallpaper, IDesktopWallpaper};
use windows::core::{PCWSTR, PWSTR};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Original {
    /// Identificador do monitor no `IDesktopWallpaper` (caminho do dispositivo).
    monitor: String,
    /// Caminho do wallpaper do usuário (vazio = cor sólida/apresentação: não restaurável).
    path: String,
    position: i32,
}

pub struct SystemWallpaperSync {
    dw: IDesktopWallpaper,
    dir: PathBuf,
    originals: Vec<Original>,
    counter: u32,
}

/// O arquivo é um snapshot nosso? (Comparação sem diferenciar maiúsculas, como o NTFS.)
fn is_ours(dir: &Path, file: &str) -> bool {
    let (dir, file) = (dir.to_string_lossy().to_lowercase(), file.to_lowercase());
    !file.is_empty() && file.starts_with(&dir)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Lê e libera uma string alocada pelo shell.
fn take_string(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string().unwrap_or_default() };
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

impl SystemWallpaperSync {
    pub fn new() -> Option<Self> {
        let dw: IDesktopWallpaper = unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL) }
            .map_err(|e| warn!(error = %e, "IDesktopWallpaper indisponível; wallpaper do sistema não será espelhado"))
            .ok()?;
        let base = crate::config::EngineConfig::config_path().parent()?.to_path_buf();
        let dir = base.join("snapshots");
        std::fs::create_dir_all(&dir).ok()?;
        let originals = std::fs::read_to_string(base.join("original_wallpapers.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let sync = Self { dw, dir, originals, counter: 0 };
        sync.prune_old_snapshots(None);
        Some(sync)
    }

    fn originals_file(&self) -> PathBuf {
        self.dir.parent().unwrap_or(Path::new(".")).join("original_wallpapers.json")
    }

    /// Identificador do monitor cujo retângulo coincide com `rect`.
    fn monitor_id_for(&self, rect: &RECT) -> Option<String> {
        unsafe {
            for i in 0..self.dw.GetMonitorDevicePathCount().ok()? {
                let id = take_string(self.dw.GetMonitorDevicePathAt(i).ok()?);
                let w = wide(&id);
                if let Ok(r) = self.dw.GetMonitorRECT(PCWSTR(w.as_ptr()))
                    && (r.left, r.top, r.right, r.bottom) == (rect.left, rect.top, rect.right, rect.bottom)
                {
                    return Some(id);
                }
            }
        }
        None
    }

    /// Guarda o wallpaper atual do monitor, uma única vez, se ainda não for um snapshot nosso.
    fn remember_original(&mut self, monitor: &str) {
        if self.originals.iter().any(|o| o.monitor == monitor) {
            return;
        }
        let w = wide(monitor);
        let (path, position) = unsafe {
            (
                self.dw.GetWallpaper(PCWSTR(w.as_ptr())).map(take_string).unwrap_or_default(),
                self.dw.GetPosition().map(|p| p.0).unwrap_or(DWPOS_FILL.0),
            )
        };
        if is_ours(&self.dir, &path) {
            return; // sobrou de uma execução que não restaurou; o original verdadeiro já foi perdido
        }
        self.originals.push(Original { monitor: monitor.to_string(), path, position });
        if let Ok(json) = serde_json::to_string_pretty(&self.originals) {
            let _ = std::fs::write(self.originals_file(), json);
        }
    }

    /// Grava `png` como wallpaper do Windows no monitor de retângulo `rect`.
    pub fn apply(&mut self, rect: &RECT, png: &[u8]) {
        let Some(monitor) = self.monitor_id_for(rect) else {
            warn!("Monitor não encontrado no IDesktopWallpaper; snapshot ignorado");
            return;
        };
        self.remember_original(&monitor);
        // Nome novo a cada vez: o Windows pode manter em cache a imagem de um caminho já usado.
        self.counter += 1;
        let tag: String = monitor.chars().filter(|c| c.is_ascii_alphanumeric()).rev().take(8).collect();
        let file = self.dir.join(format!("snap_{tag}_{}.png", self.counter));
        if let Err(e) = std::fs::write(&file, png) {
            warn!(error = %e, "Falha ao gravar snapshot do wallpaper");
            return;
        }
        let (m, f) = (wide(&monitor), wide(&file.to_string_lossy()));
        let res = unsafe {
            let _ = self.dw.SetPosition(DWPOS_FILL);
            self.dw.SetWallpaper(PCWSTR(m.as_ptr()), PCWSTR(f.as_ptr()))
        };
        match res {
            Ok(()) => {
                info!(file = %file.display(), "Wallpaper do Windows atualizado com o snapshot");
                self.prune_old_snapshots(Some(&file));
            }
            Err(e) => warn!(error = %e, "Falha ao definir o wallpaper do Windows"),
        }
    }

    /// Apaga snapshots que não estão mais em uso (mantém `keep` e os que ainda estão ativos).
    fn prune_old_snapshots(&self, keep: Option<&Path>) {
        let in_use = self.snapshots_in_use();
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return };
        for entry in rd.flatten() {
            let p = entry.path();
            let used = Some(p.as_path()) == keep || in_use.iter().any(|u| u.eq_ignore_ascii_case(&p.to_string_lossy()));
            if !used && p.extension().is_some_and(|e| e == "png") {
                let _ = std::fs::remove_file(&p);
            }
        }
    }

    fn snapshots_in_use(&self) -> Vec<String> {
        let mut v = Vec::new();
        unsafe {
            let Ok(n) = self.dw.GetMonitorDevicePathCount() else { return v };
            for i in 0..n {
                let Ok(id) = self.dw.GetMonitorDevicePathAt(i) else { continue };
                let id = take_string(id);
                let w = wide(&id);
                if let Ok(p) = self.dw.GetWallpaper(PCWSTR(w.as_ptr())) {
                    v.push(take_string(p));
                }
            }
        }
        v
    }

    /// Devolve o wallpaper do usuário, exceto nos monitores onde ele já escolheu outro.
    pub fn restore(&mut self) {
        let originals = std::mem::take(&mut self.originals);
        for o in &originals {
            let m = wide(&o.monitor);
            let current = unsafe { self.dw.GetWallpaper(PCWSTR(m.as_ptr())).map(take_string).unwrap_or_default() };
            if !is_ours(&self.dir, &current) || o.path.is_empty() {
                continue;
            }
            let p = wide(&o.path);
            unsafe {
                let _ = self.dw.SetWallpaper(PCWSTR(m.as_ptr()), PCWSTR(p.as_ptr()));
                let _ = self.dw.SetPosition(DESKTOP_WALLPAPER_POSITION(o.position));
            }
            info!(monitor = %o.monitor, path = %o.path, "Wallpaper original do Windows restaurado");
        }
        let _ = std::fs::remove_file(self.originals_file());
        self.prune_old_snapshots(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_own_snapshots_case_insensitively() {
        let dir = Path::new(r"C:\Users\Ana\AppData\Local\IsNotAEngine\snapshots");
        assert!(is_ours(dir, r"c:\users\ana\appdata\local\isnotaengine\snapshots\snap_1.png"));
        assert!(!is_ours(dir, r"C:\Users\Ana\Pictures\praia.jpg"));
        assert!(!is_ours(dir, ""));
    }

    #[test]
    fn originals_roundtrip_through_json() {
        let v = vec![Original { monitor: "\\\\?\\DISPLAY#X".into(), path: r"C:\a.jpg".into(), position: 4 }];
        let back: Vec<Original> = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(v, back);
    }
}
