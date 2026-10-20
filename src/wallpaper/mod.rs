//! Subsistema de Wallpapers: abstração de cenas, shaders procedurais e suporte a múltiplos monitores.

pub mod catalog;
pub mod factory;
pub mod image;
pub mod manager;
pub mod shader;
pub mod video;
pub mod web;

use crate::render::pipeline::ShaderConstants;
use crate::render::RenderSurface;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};
use windows::core::Result;

pub use manager::WallpaperManager;
#[allow(unused_imports)]
pub use video::VideoWallpaper;
#[allow(unused_imports)]
pub use web::WebWallpaper;


/// Trait central para qualquer tipo de wallpaper (Shader, Vídeo, Web).
pub trait Wallpaper: Send {
    /// Nome identificador do wallpaper.
    fn name(&self) -> &str;

    /// Renderiza um frame do wallpaper na superfície de destino.
    fn render(
        &self,
        context: &ID3D11DeviceContext,
        surface: &RenderSurface,
        constants: ShaderConstants,
    ) -> Result<()>;

    /// Verifica se os arquivos fonte mudaram e recarrega em tempo real (Hot-Reload).
    fn check_reload(&mut self, device: &ID3D11Device) -> Result<bool>;

    /// Notifica o wallpaper para pausar/retomar recursos em segundo plano (ex: WebView2/áudio).
    fn set_paused(&self, _paused: bool) {}

    /// Valores de parâmetros `// param:` (por nome) escolhidos pelo usuário; no-op fora de shaders.
    fn set_params(&mut self, _overrides: &[(String, f32)]) {}

    /// Se o wallpaper obedece ao fade de transição (shaders que terminam em `finish()`).
    fn supports_fade(&self) -> bool {
        false
    }

    /// Arquivo de imagem de origem, para recortar/reduzir ao espelhar em outros lugares
    /// (wallpaper do Windows, tela de bloqueio).
    fn snapshot_image(&self) -> Option<&std::path::Path> {
        None
    }

    /// Quadro RGBA já decodificado em memória (ex.: vídeo) para espelhar no wallpaper do sistema ou tela de bloqueio.
    fn snapshot_rgba(&self) -> Option<(u32, u32, Vec<u8>)> {
        None
    }

    /// Chave e código HLSL (com o header comum) para renderizar um quadro estático offscreen.
    fn snapshot_shader(&self) -> Option<(String, String)> {
        None
    }

    /// Retorna se o wallpaper é baseado em Web (HTML5/WebView2)
    fn is_web(&self) -> bool {
        false
    }

    /// Retorna referência ao WebWallpaper se for instância Web
    fn as_web(&self) -> Option<&WebWallpaper> {
        None
    }
}
