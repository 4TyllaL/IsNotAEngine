//! Implementação de wallpaper procedural baseado em shader HLSL, com suporte a hot-reloading.

use std::path::PathBuf;
use std::time::SystemTime;
use tracing::{error, info};
use windows::{
    Win32::Foundation::E_FAIL, Win32::Graphics::Direct3D::*, Win32::Graphics::Direct3D11::*,
    core::*,
};

use super::Wallpaper;
use crate::render::RenderSurface;
use crate::render::bloom::{Bloom, BloomSettings};
use crate::render::pipeline::{ShaderConstants, ShaderPipeline};

pub struct ShaderWallpaper {
    name: String,
    source_path: Option<PathBuf>,
    last_modified: Option<SystemTime>,
    pipeline: ShaderPipeline,
    bloom: Option<Bloom>,
    bloom_settings: BloomSettings,
    params: [f32; 8],
    /// Parâmetros declarados no código e os valores do usuário (reaplicados no hot-reload).
    specs: Vec<crate::render::params::ParamSpec>,
    overrides: Vec<(String, f32)>,
    tint_ref: [f32; 2],
}

impl ShaderWallpaper {
    /// Cria um wallpaper carregando o shader HLSL a partir de um arquivo no disco.
    pub fn from_file(device: &ID3D11Device, path: PathBuf) -> Result<Self> {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("shader")
            .to_string();

        let code = std::fs::read_to_string(&path)
            .map_err(|e| Error::new(E_FAIL, format!("Falha ao ler {}: {}", path.display(), e)))?;

        let pipeline = ShaderPipeline::compile(device, &super::catalog::with_common(&code), &name)?;
        let last_modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();

        info!(name = %name, path = %path.display(), "Wallpaper de shader carregado");

        Ok(Self {
            name,
            source_path: Some(path),
            last_modified,
            pipeline,
            bloom: create_bloom(device),
            bloom_settings: BloomSettings::from_source(&code),
            params: default_params(&code),
            specs: crate::render::params::parse(&code).0,
            overrides: Vec::new(),
            tint_ref: crate::render::params::retint_ref(&code),
        })
    }

    /// Cria um wallpaper a partir de código HLSL embutido no binário.
    pub fn from_embedded(device: &ID3D11Device, name: &str, code: &str) -> Result<Self> {
        let pipeline = ShaderPipeline::compile(device, &super::catalog::with_common(code), name)?;
        Ok(Self {
            name: name.to_string(),
            source_path: None,
            last_modified: None,
            pipeline,
            bloom: create_bloom(device),
            bloom_settings: BloomSettings::from_source(code),
            params: default_params(code),
            specs: crate::render::params::parse(code).0,
            overrides: Vec::new(),
            tint_ref: crate::render::params::retint_ref(code),
        })
    }
}

fn default_params(code: &str) -> [f32; 8] {
    crate::render::params::defaults(&crate::render::params::parse(code).0)
}

/// Falha no bloom não impede o wallpaper: ele só é desenhado sem o efeito.
fn create_bloom(device: &ID3D11Device) -> Option<Bloom> {
    Bloom::new(device)
        .map_err(|e| error!(error = %e, "Falha ao criar bloom; renderizando sem pós-processamento"))
        .ok()
}

impl Wallpaper for ShaderWallpaper {
    fn name(&self) -> &str {
        &self.name
    }

    fn set_params(&mut self, overrides: &[(String, f32)]) {
        self.overrides = overrides.to_vec();
        self.params = crate::render::params::resolve(&self.specs, &self.overrides);
    }

    fn render(
        &self,
        context: &ID3D11DeviceContext,
        surface: &RenderSurface,
        mut constants: ShaderConstants,
    ) -> Result<()> {
        constants.params = self.params;
        constants.tint_ref = self.tint_ref;
        self.pipeline.update_constants(context, constants)?;

        unsafe {
            let viewport = D3D11_VIEWPORT {
                Width: surface.width as f32,
                Height: surface.height as f32,
                MaxDepth: 1.0,
                ..Default::default()
            };

            let bloom = self.bloom.as_ref().filter(|_| self.bloom_settings.enabled());
            let target = match bloom {
                Some(b) => b.scene_rtv(context, surface.width, surface.height)?,
                None => surface.rtv.clone(),
            };
            context.OMSetRenderTargets(Some(&[Some(target)]), None);
            context.RSSetViewports(Some(&[viewport]));
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.pipeline.vs, None);
            context.PSSetShader(&self.pipeline.ps, None);
            context.PSSetConstantBuffers(0, Some(&[Some(self.pipeline.constants_buffer.clone())]));

            context.Draw(3, 0);
            if let Some(b) = bloom {
                b.apply(context, &surface.rtv, surface.width, surface.height, self.bloom_settings, constants.time)?;
            }
            surface
                .swap_chain
                .Present(0, windows::Win32::Graphics::Dxgi::DXGI_PRESENT(0))
                .ok()
        }
    }

    fn supports_fade(&self) -> bool {
        true
    }

    fn snapshot_shader(&self) -> Option<(String, String)> {
        let code = match &self.source_path {
            Some(p) => std::fs::read_to_string(p).ok()?,
            None => super::catalog::find(&self.name)?.source.to_string(),
        };
        Some((self.name.clone(), super::catalog::with_common(&code)))
    }

    fn check_reload(&mut self, device: &ID3D11Device) -> Result<bool> {
        let path = match &self.source_path {
            Some(p) => p,
            None => return Ok(false),
        };

        let current_modified = match std::fs::metadata(path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => return Ok(false),
        };

        if let Some(prev) = self.last_modified
            && current_modified <= prev
        {
            return Ok(false);
        }

        // Arquivo modificado: tenta recompilar em tempo real
        let code = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                error!(path = %path.display(), error = %e, "Erro ao ler arquivo modificado para hot-reload");
                return Ok(false);
            }
        };

        match ShaderPipeline::compile(device, &super::catalog::with_common(&code), &self.name) {
            Ok(new_pipeline) => {
                self.pipeline = new_pipeline;
                self.bloom_settings = BloomSettings::from_source(&code);
                self.specs = crate::render::params::parse(&code).0;
                self.params = crate::render::params::resolve(&self.specs, &self.overrides);
                self.tint_ref = crate::render::params::retint_ref(&code);
                self.last_modified = Some(current_modified);
                info!(wallpaper = %self.name, "Shader recarregado com sucesso (Hot-Reload)!");
                Ok(true)
            }
            Err(e) => {
                error!(
                    wallpaper = %self.name,
                    error = %e,
                    "Falha ao recompilar shader durante hot-reload. Mantendo versão anterior em execução."
                );
                // Não atualiza last_modified para permitir nova tentativa assim que o usuário corrigir o erro
                Ok(false)
            }
        }
    }
}
