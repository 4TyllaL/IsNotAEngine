//! Renderização offscreen de shaders para pré-visualizações do Studio.
//!
//! Usa um dispositivo D3D11 próprio (independente do loop do motor), renderiza em uma
//! textura RGBA e devolve os pixels por readback. O Studio desenha esses bytes num canvas,
//! então a prévia é exatamente o shader que roda no desktop (incluindo o bloom).

use std::collections::HashMap;
use windows::{
    Win32::Foundation::{E_FAIL, HMODULE},
    Win32::Graphics::Direct3D::*,
    Win32::Graphics::Direct3D11::*,
    Win32::Graphics::Dxgi::Common::*,
    core::*,
};

use super::bloom::{Bloom, BloomSettings};
use super::pipeline::{ShaderConstants, ShaderPipeline};

struct Target {
    texture: ID3D11Texture2D,
    rtv: ID3D11RenderTargetView,
    staging: ID3D11Texture2D,
}

pub struct OffscreenRenderer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    targets: HashMap<(u32, u32), Target>,
    pipelines: HashMap<String, Loaded>,
    bloom: Option<Bloom>,
    timer: Option<GpuTimer>,
}

struct Loaded {
    pipeline: ShaderPipeline,
    bloom: BloomSettings,
    params: [f32; 8],
    specs: Vec<super::params::ParamSpec>,
    tint_ref: [f32; 2],
}

/// Par de timestamps + disjoint para medir o tempo de GPU de um frame.
struct GpuTimer {
    disjoint: ID3D11Query,
    begin: ID3D11Query,
    end: ID3D11Query,
}

impl GpuTimer {
    fn new(device: &ID3D11Device) -> Result<Self> {
        let q = |kind| -> Result<ID3D11Query> {
            let mut out = None;
            unsafe { device.CreateQuery(&D3D11_QUERY_DESC { Query: kind, MiscFlags: 0 }, Some(&mut out))? };
            out.ok_or_else(|| Error::new(E_FAIL, "query"))
        };
        Ok(Self { disjoint: q(D3D11_QUERY_TIMESTAMP_DISJOINT)?, begin: q(D3D11_QUERY_TIMESTAMP)?, end: q(D3D11_QUERY_TIMESTAMP)? })
    }

    /// Lê o resultado (bloqueia até a GPU terminar). `None` se o clock foi disjunto.
    fn read_ms(&self, context: &ID3D11DeviceContext) -> Option<f64> {
        fn get<T: Default>(context: &ID3D11DeviceContext, q: &ID3D11Query) -> Option<T> {
            let mut v = T::default();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                // Chamada crua: o wrapper converte S_FALSE ("ainda não pronto") em Ok.
                let hr = unsafe {
                    (Interface::vtable(context).GetData)(
                        Interface::as_raw(context),
                        Interface::as_raw(q),
                        &mut v as *mut T as *mut _,
                        std::mem::size_of::<T>() as u32,
                        0,
                    )
                };
                if hr == windows::Win32::Foundation::S_OK {
                    return Some(v);
                }
                if hr.is_err() || std::time::Instant::now() > deadline {
                    return None;
                }
                std::thread::yield_now();
            }
        }
        let d: D3D11_QUERY_DATA_TIMESTAMP_DISJOINT = get(context, &self.disjoint)?;
        let a: u64 = get(context, &self.begin)?;
        let b: u64 = get(context, &self.end)?;
        (!d.Disjoint.as_bool() && d.Frequency > 0).then(|| (b.saturating_sub(a)) as f64 * 1000.0 / d.Frequency as f64)
    }
}

impl OffscreenRenderer {
    pub fn new() -> Result<Self> {
        unsafe {
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(0),
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            Ok(Self {
                device: device.ok_or_else(|| Error::new(E_FAIL, "sem D3D11Device"))?,
                context: context.ok_or_else(|| Error::new(E_FAIL, "sem contexto D3D11"))?,
                targets: HashMap::new(),
                pipelines: HashMap::new(),
                bloom: None,
                timer: None,
            })
        }
    }

    fn target(&mut self, w: u32, h: u32) -> Result<&Target> {
        if !self.targets.contains_key(&(w, h)) {
            unsafe {
                let mut desc = D3D11_TEXTURE2D_DESC {
                    Width: w,
                    Height: h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                    ..Default::default()
                };
                let mut texture = None;
                self.device.CreateTexture2D(&desc, None, Some(&mut texture))?;
                let texture = texture.ok_or_else(|| Error::new(E_FAIL, "textura"))?;
                let mut rtv = None;
                self.device.CreateRenderTargetView(&texture, None, Some(&mut rtv))?;

                desc.Usage = D3D11_USAGE_STAGING;
                desc.BindFlags = 0;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                let mut staging = None;
                self.device.CreateTexture2D(&desc, None, Some(&mut staging))?;

                self.targets.insert(
                    (w, h),
                    Target {
                        texture,
                        rtv: rtv.ok_or_else(|| Error::new(E_FAIL, "rtv"))?,
                        staging: staging.ok_or_else(|| Error::new(E_FAIL, "staging"))?,
                    },
                );
            }
        }
        Ok(&self.targets[&(w, h)])
    }

    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    /// Registra (ou substitui) um pipeline já compilado. `code` é a fonte do shader,
    /// de onde saem a diretiva `// bloom:` e os padrões de `// param:`.
    pub fn insert(&mut self, key: &str, pipeline: ShaderPipeline, code: &str) {
        // Com o header comum prefixado (`with_common`), as diretivas ficam depois do `#line 1`:
        // lê só a fonte do shader, senão elas caem fora das 40 linhas inspecionadas.
        let code = code.split_once("\n#line 1\n").map_or(code, |(_, src)| src);
        let specs = super::params::parse(code).0;
        let params = super::params::defaults(&specs);
        let bloom = BloomSettings::from_source(code);
        let tint_ref = super::params::retint_ref(code);
        self.pipelines.insert(key.to_string(), Loaded { pipeline, bloom, params, specs, tint_ref });
    }

    /// Descarta um pipeline registrado (a próxima `render` com a chave recompila).
    pub fn forget(&mut self, key: &str) {
        self.pipelines.remove(key);
    }

    /// Compila (com cache por chave) e renderiza um frame, devolvendo pixels RGBA8.
    /// Os parâmetros recebem os padrões declarados no shader.
    pub fn render(&mut self, key: &str, code: &str, constants: ShaderConstants) -> Result<Vec<u8>> {
        self.render_with_params(key, code, constants, None)
    }

    /// Como [`Self::render`], mas com valores de parâmetros por nome (limitados à faixa declarada);
    /// `None` usa os padrões. É o mesmo cálculo que o desktop faz (`params::resolve`).
    pub fn render_with_params(&mut self, key: &str, code: &str, mut constants: ShaderConstants, overrides: Option<&[(String, f32)]>) -> Result<Vec<u8>> {
        if !self.pipelines.contains_key(key) {
            let pipeline = ShaderPipeline::compile(&self.device, code, key)?;
            self.insert(key, pipeline, code);
        }
        let loaded = &self.pipelines[key];
        constants.params = match overrides {
            Some(o) => super::params::resolve(&loaded.specs, o),
            None => loaded.params,
        };
        Ok(self.draw(key, constants, false)?.0)
    }

    /// Renderiza um pipeline registrado com as constantes exatas e mede o tempo de GPU.
    pub fn render_timed(&mut self, key: &str, constants: ShaderConstants) -> Result<(Vec<u8>, Option<f64>)> {
        self.draw(key, constants, true)
    }

    fn draw(&mut self, key: &str, mut constants: ShaderConstants, timed: bool) -> Result<(Vec<u8>, Option<f64>)> {
        let (w, h) = (constants.resolution[0] as u32, constants.resolution[1] as u32);
        self.target(w, h)?;
        if let Some(l) = self.pipelines.get(key) {
            constants.tint_ref = l.tint_ref;
        }
        let settings = self.pipelines.get(key).ok_or_else(|| Error::new(E_FAIL, "pipeline não registrado"))?.bloom;
        if settings.enabled() && self.bloom.is_none() {
            self.bloom = Some(Bloom::new(&self.device)?);
        }
        if timed && self.timer.is_none() {
            self.timer = Some(GpuTimer::new(&self.device)?);
        }
        let target = &self.targets[&(w, h)];
        let pipeline = &self.pipelines[key].pipeline;
        let bloom = self.bloom.as_ref().filter(|_| settings.enabled());
        let timer = self.timer.as_ref().filter(|_| timed);

        unsafe {
            if let Some(t) = timer {
                self.context.Begin(&t.disjoint);
                self.context.End(&t.begin);
            }
            pipeline.update_constants(&self.context, constants)?;
            let scene = match bloom {
                Some(b) => b.scene_rtv(&self.context, w, h)?,
                None => target.rtv.clone(),
            };
            let viewport = D3D11_VIEWPORT {
                Width: w as f32,
                Height: h as f32,
                MaxDepth: 1.0,
                ..Default::default()
            };
            self.context.OMSetRenderTargets(Some(&[Some(scene)]), None);
            self.context.RSSetViewports(Some(&[viewport]));
            self.context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.context.VSSetShader(&pipeline.vs, None);
            self.context.PSSetShader(&pipeline.ps, None);
            self.context
                .PSSetConstantBuffers(0, Some(&[Some(pipeline.constants_buffer.clone())]));
            self.context.Draw(3, 0);
            if let Some(b) = bloom {
                b.apply(&self.context, &target.rtv, w, h, settings, constants.time)?;
            }
            if let Some(t) = timer {
                self.context.End(&t.end);
                self.context.End(&t.disjoint);
            }
            let gpu_ms = timer.and_then(|t| t.read_ms(&self.context));
            self.context.CopyResource(&target.staging, &target.texture);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&target.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let row = (w * 4) as usize;
            let mut out = Vec::with_capacity(row * h as usize);
            for y in 0..h as usize {
                let src = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize);
                out.extend_from_slice(std::slice::from_raw_parts(src, row));
            }
            self.context.Unmap(&target.staging, 0);
            Ok((out, gpu_ms))
        }
    }
}

/// Codifica pixels RGBA8 como PNG (compressão rápida: serve de wallpaper do sistema).
pub fn encode_png(rgba: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    // Alfa do shader não é usado pelo desktop: grava opaco.
    let opaque: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect();
    enc.write_header().ok()?.write_image_data(&opaque).ok()?;
    Some(out)
}

/// Codifica pixels RGBA8 como BMP 24 bits (usado pelo modo `--render-previews`).
pub fn encode_bmp(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let row = ((w * 3 + 3) & !3) as usize;
    let size = 54 + row * h as usize;
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    for y in (0..h as usize).rev() {
        let start = out.len();
        for x in 0..w as usize {
            let i = (y * w as usize + x) * 4;
            out.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i]]);
        }
        out.resize(start + row, 0);
    }
    out
}

/// Constantes usadas nas prévias: relógio real e um áudio sintético para os
/// wallpapers reativos não ficarem parados quando não há música tocando.
pub fn preview_constants(w: u32, h: u32, t: f32, tint: [f32; 4]) -> ShaderConstants {
    let beat = (t * 2.1).fract();
    let kick = (-beat * 6.0).exp();
    let wobble = |k: f32| 0.5 + 0.5 * (t * k).sin();
    let mut spectrum = [0f32; 16];
    for (i, s) in spectrum.iter_mut().enumerate() {
        let f = i as f32 / 15.0;
        let env = (1.0 - f * 0.7) * (0.35 + 0.65 * wobble(1.3 + f * 4.0 + i as f32 * 0.37));
        *s = (env * if i < 3 { 0.5 + 0.5 * kick } else { 0.8 }).clamp(0.0, 1.0);
    }
    let (clock_time, clock_date) = crate::engine::system_clock();
    ShaderConstants {
        resolution: [w as f32, h as f32],
        time: t,
        speed: 1.0,
        tint,
        mouse: [0.5, 0.5],
        clock_time,
        clock_date,
        audio_reactive: [0.25 + 0.6 * kick, 0.4 * wobble(1.7), 0.3 * wobble(3.1), 0.45 + 0.2 * kick],
        audio_spectrum: spectrum,
        ..Default::default()
    }
}
