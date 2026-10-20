//! Módulo Desktop: gerenciamento de integração com o Windows Explorer,
//! enumeração de monitores, detecção de oclusão e escuta de eventos do sistema.

pub mod autostart;
pub mod discovery;
pub mod listener;
pub mod lock_screen;
pub mod monitors;
pub mod occlusion;
pub mod screensaver;
pub mod system_wallpaper;
pub mod tray;

pub use autostart::{is_autostart_enabled, set_autostart_enabled};
pub use discovery::{WallpaperLayer, find_wallpaper_layer, repaint_desktop};
pub use listener::DesktopEventListener;
pub use monitors::{MonitorInfo, MonitorManager};
pub use occlusion::OcclusionDetector;
pub use system_wallpaper::SystemWallpaperSync;
pub use tray::SystemTray;

