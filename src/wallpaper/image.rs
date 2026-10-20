//! Wallpaper de imagem estática (PNG, JPG, BMP, WebP, TIFF), decodificado pelo WIC do Windows.
//!
//! A imagem sobe uma vez para a GPU (com mipmaps, para não serrilhar ao reduzir um 4K) e só é
//! redesenhada quando algo muda: tamanho da superfície, fade da playlist ou, por garantia,
//! uma vez por segundo. O ajuste é "preencher": cobre a tela sem distorcer, cortando as bordas.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tracing::info;
use windows::{
    Win32::Foundation::*,
    Win32::Graphics::Direct3D::*,
    Win32::Graphics::Direct3D11::*,
    Win32::Graphics::Dxgi::Common::*,
    Win32::Graphics::Imaging::*,
    Win32::System::Com::*,
    core::*,
};

use super::Wallpaper;
use crate::render::RenderSurface;
use crate::render::pipeline::ShaderConstants;

pub const IMAGE_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "bmp", "webp", "tif", "tiff"];

/// Maior lado e maior área aceitos na GPU. Fotos maiores são reduzidas ao carregar.
const MAX_SIDE: u32 = 8192;
const MAX_PIXELS: u64 = 7680 * 4320;
/// Redesenho de segurança: custa um triângulo de tela cheia por segundo.
const REDRAW_EVERY: Duration = Duration::from_secs(1);

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Fator (<= 1) que faz uma imagem `w`x`h` caber nos limites da GPU, mantendo a proporção.
pub fn fit_scale(w: u32, h: u32) -> f64 {
    let (w, h) = (f64::from(w.max(1)), f64::from(h.max(1)));
    let by_side = f64::from(MAX_SIDE) / w.max(h);
    let by_area = (MAX_PIXELS as f64 / (w * h)).sqrt();
    by_side.min(by_area).min(1.0)
}

/// Retângulo de corte (x, y, largura, altura) que faz uma imagem `iw`x`ih` cobrir `sw`x`sh`
/// sem distorcer, centralizado.
pub fn cover_crop(iw: u32, ih: u32, sw: u32, sh: u32) -> (u32, u32, u32, u32) {
    let (iw, ih, sw, sh) = (iw.max(1), ih.max(1), sw.max(1), sh.max(1));
    // compara iw/ih com sw/sh sem divisão
    if u64::from(iw) * u64::from(sh) > u64::from(sw) * u64::from(ih) {
        let cw = (u64::from(ih) * u64::from(sw) / u64::from(sh)).clamp(1, u64::from(iw)) as u32;
        ((iw - cw) / 2, 0, cw, ih)
    } else {
        let ch = (u64::from(iw) * u64::from(sh) / u64::from(sw)).clamp(1, u64::from(ih)) as u32;
        (0, (ih - ch) / 2, iw, ch)
    }
}

pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

fn factory() -> Result<IWICImagingFactory> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
    }
}

/// Orientação EXIF (1..8) do primeiro quadro, se houver.
fn exif_orientation(frame: &IWICBitmapFrameDecode) -> u16 {
    unsafe {
        let Ok(reader) = frame.GetMetadataQueryReader() else { return 1 };
        let mut value = windows::Win32::System::Com::StructuredStorage::PROPVARIANT::default();
        if reader.GetMetadataByName(w!("/app1/ifd/{ushort=274}"), &mut value).is_err() {
            return 1;
        }
        u16::try_from(&value).ok().filter(|o| (1..=8).contains(o)).unwrap_or(1)
    }
}

fn orientation_transform(orientation: u16) -> Option<WICBitmapTransformOptions> {
    Some(match orientation {
        2 => WICBitmapTransformFlipHorizontal,
        3 => WICBitmapTransformRotate180,
        4 => WICBitmapTransformFlipVertical,
        5 => WICBitmapTransformOptions(WICBitmapTransformRotate270.0 | WICBitmapTransformFlipHorizontal.0),
        6 => WICBitmapTransformRotate90,
        7 => WICBitmapTransformOptions(WICBitmapTransformRotate90.0 | WICBitmapTransformFlipHorizontal.0),
        8 => WICBitmapTransformRotate270,
        _ => return None,
    })
}

/// Abre o arquivo como uma fonte BGRA 32 bits, já na orientação certa.
fn open_source(factory: &IWICImagingFactory, path: &Path) -> Result<IWICBitmapSource> {
    unsafe {
        let decoder = factory.CreateDecoderFromFilename(
            &HSTRING::from(path.as_os_str()),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )?;
        let frame = decoder.GetFrame(0)?;
        let orientation = exif_orientation(&frame);

        let converter = factory.CreateFormatConverter()?;
        converter.Initialize(&frame, &GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
        let mut source: IWICBitmapSource = converter.cast()?;

        if let Some(options) = orientation_transform(orientation) {
            let rotator = factory.CreateBitmapFlipRotator()?;
            rotator.Initialize(&source, options)?;
            source = rotator.cast()?;
        }
        Ok(source)
    }
}

fn read_pixels(source: &IWICBitmapSource, w: u32, h: u32) -> Result<Vec<u8>> {
    let stride = w * 4;
    let mut buf = vec![0u8; stride as usize * h as usize];
    unsafe { source.CopyPixels(std::ptr::null(), stride, &mut buf)? };
    Ok(buf)
}

/// Decodifica a imagem, reduzindo-a se passar dos limites da GPU.
pub fn decode(path: &Path) -> Result<Decoded> {
    let factory = factory()?;
    let mut source = open_source(&factory, path)?;
    let (mut w, mut h) = (0u32, 0u32);
    unsafe { source.GetSize(&mut w, &mut h)? };
    let scale = fit_scale(w, h);
    if scale < 1.0 {
        let (nw, nh) = (((f64::from(w) * scale) as u32).max(1), ((f64::from(h) * scale) as u32).max(1));
        unsafe {
            let scaler = factory.CreateBitmapScaler()?;
            scaler.Initialize(&source, nw, nh, WICBitmapInterpolationModeHighQualityCubic)?;
            source = scaler.cast()?;
        }
        (w, h) = (nw, nh);
    }
    let bgra = read_pixels(&source, w, h)?;
    Ok(Decoded { width: w, height: h, bgra })
}

/// A imagem recortada para cobrir `w`x`h` e reduzida/ampliada a esse tamanho, em RGBA.
/// Usada para espelhar no wallpaper do Windows e na tela de bloqueio.
pub fn cover_rgba(path: &Path, w: u32, h: u32) -> Result<Vec<u8>> {
    let factory = factory()?;
    let source = open_source(&factory, path)?;
    let (mut iw, mut ih) = (0u32, 0u32);
    unsafe { source.GetSize(&mut iw, &mut ih)? };
    let (x, y, cw, ch) = cover_crop(iw, ih, w, h);
    let bgra = unsafe {
        let clipper = factory.CreateBitmapClipper()?;
        clipper.Initialize(&source, &WICRect { X: x as i32, Y: y as i32, Width: cw as i32, Height: ch as i32 })?;
        let scaler = factory.CreateBitmapScaler()?;
        scaler.Initialize(&clipper, w, h, WICBitmapInterpolationModeHighQualityCubic)?;
        let scaled: IWICBitmapSource = scaler.cast()?;
        read_pixels(&scaled, w, h)?
    };
    Ok(bgra_to_rgba(bgra))
}

fn bgra_to_rgba(mut px: Vec<u8>) -> Vec<u8> {
    for p in px.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    px
}

const IMAGE_HLSL: &str = r#"
struct VSOutput { float4 pos : SV_POSITION; float2 uv : TEXCOORD0; };

VSOutput vs_main(uint id : SV_VertexID) {
    VSOutput o;
    o.uv = float2((id << 1) & 2, id & 2);
    o.pos = float4(o.uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return o;
}

cbuffer Params : register(b0) {
    float2 surf;   // tamanho da superfície
    float2 img;    // tamanho da imagem
    float4 fade;   // x = escurecimento da transição da playlist
};
Texture2D tex : register(t0);
SamplerState smp : register(s0);

float4 ps_main(VSOutput i) : SV_TARGET {
    float sa = surf.x / surf.y, ia = img.x / img.y;
    float2 scale = sa > ia ? float2(1.0, ia / sa) : float2(sa / ia, 1.0);   // "preencher"
    float2 uv = (i.uv - 0.5) * scale + 0.5;
    return float4(tex.Sample(smp, uv).rgb * (1.0 - saturate(fade.x)), 1.0);
}
"#;

pub struct ImageWallpaper {
    name: String,
    path: PathBuf,
    width: u32,
    height: u32,
    _texture: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    sampler: ID3D11SamplerState,
    params: ID3D11Buffer,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    draws: Cell<u32>,
    last_size: Cell<(u32, u32)>,
    last_fade: Cell<f32>,
    last_draw: Cell<Option<Instant>>,
}

unsafe impl Send for ImageWallpaper {}

impl ImageWallpaper {
    pub fn from_file(device: &ID3D11Device, path: PathBuf) -> Result<Self> {
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("imagem").to_string();
        let decoded = decode(&path)?;
        unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: decoded.width,
                Height: decoded.height,
                MipLevels: 0, // 0 = cadeia completa de mipmaps
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32,
            };
            let mut texture = None;
            device.CreateTexture2D(&desc, None, Some(&mut texture))?;
            let texture = texture.ok_or_else(|| Error::new(E_FAIL, "sem textura"))?;
            let mut srv = None;
            device.CreateShaderResourceView(&texture, None, Some(&mut srv))?;
            let srv = srv.ok_or_else(|| Error::new(E_FAIL, "sem SRV"))?;

            let context = device.GetImmediateContext()?;
            context.UpdateSubresource(&texture, 0, None, decoded.bgra.as_ptr() as *const _, decoded.width * 4, 0);
            context.GenerateMips(&srv);

            let sampler_desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                MinLOD: 0.0,
                MaxLOD: D3D11_FLOAT32_MAX,
                ..Default::default()
            };
            let mut sampler = None;
            device.CreateSamplerState(&sampler_desc, Some(&mut sampler))?;

            let buffer_desc = D3D11_BUFFER_DESC {
                ByteWidth: 32,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                ..Default::default()
            };
            let mut params = None;
            device.CreateBuffer(&buffer_desc, None, Some(&mut params))?;

            let vs_bytes = crate::render::pipeline::get_or_compile_bytecode(IMAGE_HLSL, "image_vs", s!("vs_main"), s!("vs_5_0"))?;
            let ps_bytes = crate::render::pipeline::get_or_compile_bytecode(IMAGE_HLSL, "image_ps", s!("ps_main"), s!("ps_5_0"))?;
            let (mut vs, mut ps) = (None, None);
            device.CreateVertexShader(&vs_bytes, None, Some(&mut vs))?;
            device.CreatePixelShader(&ps_bytes, None, Some(&mut ps))?;

            info!(name = %name, width = decoded.width, height = decoded.height, "Wallpaper de imagem carregado");
            Ok(Self {
                name,
                path,
                width: decoded.width,
                height: decoded.height,
                _texture: texture,
                srv,
                sampler: sampler.ok_or_else(|| Error::new(E_FAIL, "sem sampler"))?,
                params: params.ok_or_else(|| Error::new(E_FAIL, "sem buffer"))?,
                vs: vs.ok_or_else(|| Error::new(E_FAIL, "sem VS"))?,
                ps: ps.ok_or_else(|| Error::new(E_FAIL, "sem PS"))?,
                draws: Cell::new(0),
                last_size: Cell::new((0, 0)),
                last_fade: Cell::new(0.0),
                last_draw: Cell::new(None),
            })
        }
    }

    fn needs_draw(&self, size: (u32, u32), fade: f32) -> bool {
        self.draws.get() < 3 // cobre os dois buffers da swap chain, com folga
            || size != self.last_size.get()
            || fade != self.last_fade.get()
            || self.last_draw.get().is_none_or(|t| t.elapsed() >= REDRAW_EVERY)
    }
}

impl Wallpaper for ImageWallpaper {
    fn name(&self) -> &str {
        &self.name
    }

    fn render(&self, context: &ID3D11DeviceContext, surface: &RenderSurface, constants: ShaderConstants) -> Result<()> {
        let size = (surface.width, surface.height);
        let fade = constants.fade[0];
        if !self.needs_draw(size, fade) {
            return Ok(());
        }
        self.draw(context, &surface.rtv, size, fade)?;
        unsafe { surface.swap_chain.Present(0, windows::Win32::Graphics::Dxgi::DXGI_PRESENT(0)).ok()? };
        self.draws.set(self.draws.get().saturating_add(1));
        self.last_size.set(size);
        self.last_fade.set(fade);
        self.last_draw.set(Some(Instant::now()));
        Ok(())
    }

    fn check_reload(&mut self, _device: &ID3D11Device) -> Result<bool> {
        Ok(false)
    }

    fn supports_fade(&self) -> bool {
        true
    }

    fn snapshot_image(&self) -> Option<&Path> {
        Some(&self.path)
    }
}

impl ImageWallpaper {
    /// Desenha a imagem em `rtv` (sem apresentar): "preencher" mais o fade da playlist.
    fn draw(&self, context: &ID3D11DeviceContext, rtv: &ID3D11RenderTargetView, size: (u32, u32), fade: f32) -> Result<()> {
        unsafe {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&self.params, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))?;
            let values = [size.0 as f32, size.1 as f32, self.width as f32, self.height as f32, fade, 0.0, 0.0, 0.0];
            (mapped.pData as *mut [f32; 8]).write(values);
            context.Unmap(&self.params, 0);

            let viewport = D3D11_VIEWPORT { Width: size.0 as f32, Height: size.1 as f32, MaxDepth: 1.0, ..Default::default() };
            context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            context.RSSetViewports(Some(&[viewport]));
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vs, None);
            context.PSSetShader(&self.ps, None);
            context.PSSetConstantBuffers(0, Some(&[Some(self.params.clone())]));
            context.PSSetShaderResources(0, Some(&[Some(self.srv.clone())]));
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            context.Draw(3, 0);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_image_extensions() {
        assert!(is_image(Path::new("a/b/foto.JPG")));
        assert!(is_image(Path::new("x.webp")));
        assert!(!is_image(Path::new("x.mp4")));
        assert!(!is_image(Path::new("semextensao")));
    }

    #[test]
    fn big_photos_shrink_to_gpu_limits_keeping_aspect() {
        assert_eq!(fit_scale(3840, 2160), 1.0);
        assert_eq!(fit_scale(7680, 4320), 1.0);
        let s = fit_scale(12000, 8000); // foto de 96 MP
        assert!(s < 1.0);
        let (w, h) = (12000.0 * s, 8000.0 * s);
        assert!(w <= f64::from(MAX_SIDE) + 1.0 && w * h <= MAX_PIXELS as f64 + 1.0e5, "{w}x{h}");
        assert!(fit_scale(20000, 100) < 0.5); // faixa estreita: o limite é o lado
    }

    #[test]
    fn cover_crop_keeps_the_screen_aspect_and_centers() {
        // imagem 16:9 em tela 21:9: corta em cima e embaixo
        let (x, y, w, h) = cover_crop(3840, 2160, 2560, 1080);
        assert_eq!((x, w), (0, 3840));
        assert_eq!(h, 1620);
        assert_eq!(y, (2160 - 1620) / 2);
        // imagem 21:9 em tela 16:9: corta dos lados
        let (x, y, w, h) = cover_crop(2560, 1080, 1920, 1080);
        assert_eq!((y, h), (0, 1080));
        assert_eq!(w, 1920);
        assert_eq!(x, (2560 - 1920) / 2);
        // mesma proporção: não corta
        assert_eq!(cover_crop(1920, 1080, 3840, 2160), (0, 0, 1920, 1080));
        // entradas degeneradas não entram em pânico
        let _ = cover_crop(0, 0, 0, 0);
    }

    fn write_png(path: &Path, w: u32, h: u32, rgba: impl Fn(u32, u32) -> [u8; 4]) {
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                px.extend_from_slice(&rgba(x, y));
            }
        }
        let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path).unwrap()), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&px).unwrap();
    }

    #[test]
    fn decodes_png_through_wic() {
        let dir = std::env::temp_dir().join("isnotaengine_image_test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("cores.png");
        // metade esquerda vermelha, direita azul
        write_png(&file, 8, 4, |x, _| if x < 4 { [255, 0, 0, 255] } else { [0, 0, 255, 255] });
        let d = decode(&file).expect("WIC decodifica PNG");
        assert_eq!((d.width, d.height), (8, 4));
        assert_eq!(&d.bgra[0..4], &[0, 0, 255, 255], "pixel (0,0) vermelho em BGRA");
        let right = (7 * 4) as usize;
        assert_eq!(&d.bgra[right..right + 4], &[255, 0, 0, 255], "pixel (7,0) azul em BGRA");
    }

    #[test]
    fn cover_rgba_crops_and_converts_to_rgba() {
        let dir = std::env::temp_dir().join("isnotaengine_image_test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("faixas.png");
        // 16x4: esquerda e direita verdes, centro vermelho. Em tela quadrada o corte central fica vermelho.
        write_png(&file, 16, 4, |x, _| if (6..10).contains(&x) { [255, 0, 0, 255] } else { [0, 255, 0, 255] });
        let px = cover_rgba(&file, 4, 4).expect("recorte");
        assert_eq!(px.len(), 4 * 4 * 4);
        assert_eq!(&px[0..4], &[255, 0, 0, 255], "o canto do recorte é vermelho, em RGBA");
    }

    /// Desenha `wp` num alvo offscreen `w`x`h` e devolve os pixels RGBA.
    fn draw_offscreen(wp: &ImageWallpaper, device: &ID3D11Device, w: u32, h: u32, fade: f32) -> Vec<u8> {
        unsafe {
            let context = device.GetImmediateContext().unwrap();
            let desc = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                ..Default::default()
            };
            let mut target = None;
            device.CreateTexture2D(&desc, None, Some(&mut target)).unwrap();
            let target = target.unwrap();
            let mut rtv = None;
            device.CreateRenderTargetView(&target, None, Some(&mut rtv)).unwrap();
            wp.draw(&context, &rtv.unwrap(), (w, h), fade).unwrap();

            let staging_desc = D3D11_TEXTURE2D_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                ..desc
            };
            let mut staging = None;
            device.CreateTexture2D(&staging_desc, None, Some(&mut staging)).unwrap();
            let staging = staging.unwrap();
            context.CopyResource(&staging, &target);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
            let mut out = Vec::new();
            for y in 0..h as usize {
                let row = std::slice::from_raw_parts((mapped.pData as *const u8).add(y * mapped.RowPitch as usize), w as usize * 4);
                out.extend_from_slice(row);
            }
            context.Unmap(&staging, 0);
            bgra_to_rgba(out)
        }
    }

    fn test_device() -> ID3D11Device {
        unsafe {
            let mut device = None;
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0), None, D3D11_SDK_VERSION, Some(&mut device), None, None)
                .expect("GPU");
            device.unwrap()
        }
    }

    #[test]
    fn gpu_draw_fills_without_distortion_and_fades() {
        let dir = std::env::temp_dir().join("isnotaengine_image_test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("gpu.png");
        // 16x4: verde nos extremos, vermelho no centro (x 6..10)
        write_png(&file, 16, 4, |x, _| if (6..10).contains(&x) { [255, 0, 0, 255] } else { [0, 255, 0, 255] });
        let device = test_device();
        let wp = ImageWallpaper::from_file(&device, file).expect("imagem na GPU");

        // Tela quadrada 8x8: "preencher" recorta o centro 4x4 da imagem, que é todo vermelho
        let px = draw_offscreen(&wp, &device, 8, 8, 0.0);
        let at = |x: usize, y: usize| &px[(y * 8 + x) * 4..(y * 8 + x) * 4 + 3];
        for (x, y) in [(2, 2), (5, 2), (3, 4), (5, 5)] {
            let p = at(x, y);
            assert!(p[0] > 240 && p[1] < 15 && p[2] < 15, "({x},{y}) deveria ser vermelho: {p:?}");
        }

        // Tela larga 16x4 (mesma proporção): a imagem aparece inteira, com as bordas verdes
        let wide = draw_offscreen(&wp, &device, 16, 4, 0.0);
        let p = |x: usize| &wide[(x * 4)..(x * 4 + 3)];
        assert!(p(1)[1] > 240 && p(1)[0] < 15, "borda esquerda verde: {:?}", p(1));
        assert!(p(8)[0] > 240 && p(8)[1] < 15, "centro vermelho: {:?}", p(8));

        // Fade total vira preto; meio fade escurece pela metade
        let black = draw_offscreen(&wp, &device, 8, 8, 1.0);
        assert!(black.as_chunks::<4>().0.iter().all(|p| p[0] < 3 && p[1] < 3 && p[2] < 3), "fade 1.0 = preto");
        let half = draw_offscreen(&wp, &device, 8, 8, 0.5);
        let mid = (3 * 8 + 3) * 4;
        assert!((124..=131).contains(&half[mid]), "fade 0.5 = metade do vermelho: {}", half[mid]);
    }

    #[test]
    fn frozen_shader_roundtrips_through_a_4k_png() {
        use crate::render::offscreen::{OffscreenRenderer, encode_png, preview_constants};
        let (key, code) = crate::wallpaper::WallpaperManager::shader_source("gyroid_silk").expect("shader do catálogo");
        let (w, h) = (3840u32, 2160u32);
        let started = Instant::now();
        let px = OffscreenRenderer::new()
            .expect("GPU")
            .render(&key, &code, preview_constants(w, h, 9.0, [0.0; 4]))
            .expect("render 4K");
        let png = encode_png(&px, w, h).expect("PNG");
        let file = std::env::temp_dir().join("isnotaengine_image_test").join("congelado_4k.png");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, &png).unwrap();
        let rendered = started.elapsed();

        let device = test_device();
        let wp = ImageWallpaper::from_file(&device, file).expect("PNG 4K vira wallpaper de imagem");
        assert_eq!((wp.width, wp.height), (w, h));
        // mostrado em 1080p, o conteúdo bate com o 4K reduzido (mipmaps): compara o brilho médio
        let small = draw_offscreen(&wp, &device, 480, 270, 0.0);
        let mean = |v: &[u8]| v.as_chunks::<4>().0.iter().map(|p| (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) as f64 / 3.0).sum::<f64>() / (v.len() / 4) as f64;
        let (a, b) = (mean(&px), mean(&small));
        assert!((a - b).abs() < 4.0, "brilho médio 4K {a:.1} vs reduzido {b:.1}");
        assert!(rendered < Duration::from_secs(20), "renderizar e gravar 4K demorou {rendered:?}");
    }

    #[test]
    fn missing_file_is_an_error_not_a_panic() {
        assert!(decode(Path::new("C:/nao/existe/foto.png")).is_err());
    }
}
