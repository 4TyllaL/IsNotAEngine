//! Módulo de Renderização: contexto D3D11, swapchains e shaders.

pub mod bloom;
pub mod gpu;
pub mod offscreen;
pub mod params;
pub mod pipeline;
pub mod surface;

pub use gpu::GpuContext;
pub use surface::RenderSurface;
