//! Decodificador de papéis de parede em vídeo com aceleração de hardware via Windows Media Foundation (WMF).

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tracing::{error, info};
use windows::{
    Win32::Foundation::*,
    Win32::Graphics::Direct3D::Fxc::*,
    Win32::Graphics::Direct3D::*,
    Win32::Graphics::Direct3D11::*,
    Win32::Graphics::Dxgi::Common::*,
    Win32::Media::MediaFoundation::*,
    Win32::System::Com::StructuredStorage::PROPVARIANT,
    Win32::System::Com::*,
    core::*,
};

use super::Wallpaper;
use crate::render::RenderSurface;
use crate::render::pipeline::ShaderConstants;

const VIDEO_HLSL: &str = r#"
cbuffer VideoConstants : register(b0) {
    float4 fade;   // x = escurecimento da playlist; y = mistura com o começo do vídeo (loop sem corte)
};

struct VSOutput {
    float4 pos : SV_POSITION;
    float2 uv : TEXCOORD0;
};

VSOutput vs_main(uint id : SV_VertexID) {
    VSOutput output;
    output.uv = float2((id << 1) & 2, id & 2);
    output.pos = float4(output.uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return output;
}

Texture2D videoTexture : register(t0);   // quadro atual
Texture2D videoNext : register(t1);      // cópia do começo, entrando no fim do loop
SamplerState videoSampler : register(s0);

float4 ps_main(VSOutput input) : SV_TARGET {
    float4 c = lerp(videoTexture.Sample(videoSampler, input.uv), videoNext.Sample(videoSampler, input.uv), saturate(fade.y));
    return float4(c.rgb * (1.0 - saturate(fade.x)), 1.0);
}
"#;

pub struct VideoWallpaper {
    name: String,
    _path: PathBuf,
    width: u32,
    height: u32,
    frame_interval: Duration,
    last_frame_time: RefCell<Instant>,
    last_bgra: RefCell<Vec<u8>>,
    /// Trocado por um novo leitor quando o rebobinar falha ou o vídeo trava (ver `advance_frame`).
    reader: RefCell<IMFSourceReader>,
    /// Último instante em que um quadro novo chegou à textura (vigia contra travamento).
    last_decoded: Cell<Instant>,
    /// Quantas vezes o vídeo voltou ao começo (só para o log).
    loops: Cell<u32>,
    /// Duas texturas: a do quadro atual (`prim`) e a da cópia do começo que entra no fim do loop.
    textures: [ID3D11Texture2D; 2],
    srvs: [ID3D11ShaderResourceView; 2],
    prim: Cell<usize>,
    /// Segundo leitor, posicionado no começo, que roda junto com o fim do vídeo durante o fade.
    secondary: RefCell<Option<IMFSourceReader>>,
    secondary_started: Cell<bool>,
    /// Duração do vídeo e do fade, em unidades de 100 ns (fade 0 = loop com corte, para vídeos muito curtos).
    duration: i64,
    fade_len: i64,
    /// 0 = só o quadro atual, 1 = só a cópia do começo.
    blend: Cell<f32>,
    sampler: ID3D11SamplerState,
    cbuffer: ID3D11Buffer,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
}


unsafe impl Send for VideoWallpaper {}

/// Abre o arquivo no Media Foundation com saída RGB32 (decodificação por hardware quando o sistema oferece).
fn open_reader(path: &std::path::Path) -> Result<IMFSourceReader> {
    unsafe {
        let mut attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attributes, 4)?;
        if let Some(ref attr) = attributes {
            let _ = attr.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1);
            let _ = attr.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1);
        }
        let hpath = HSTRING::from(path.to_str().unwrap_or_default());
        let reader: IMFSourceReader = MFCreateSourceReaderFromURL(&hpath, attributes.as_ref())?;

        // Configura saída de vídeo para RGB32 descompactado
        let partial_type = MFCreateMediaType()?;
        partial_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        partial_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        reader.SetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, None, &partial_type)?;
        Ok(reader)
    }
}

/// Sem quadro novo por tanto tempo, o leitor é considerado travado e reaberto.
const STALL_LIMIT: Duration = Duration::from_secs(2);
/// Quanto do fim do vídeo se mistura com o começo (100 ns): 1,5 s. Em vídeos curtos, no máximo 1/4 da duração.
const CROSSFADE: i64 = 15_000_000;
/// Abaixo disso (3 s) o fade comeria o vídeo todo e o loop volta ao corte simples.
const MIN_LOOP_FOR_FADE: i64 = 30_000_000;
/// O leitor da cópia do começo é aberto um pouco antes, para a abertura não pesar no instante do fade.
const WARMUP: i64 = 10_000_000;

/// Resultado de ler um quadro de um leitor.
enum Read {
    Frame(i64),
    End,
    Gap,
    Failed,
}

impl VideoWallpaper {
    /// Volta ao começo para o loop. O tempo é um `VT_I8` em unidades de 100 ns (um `PROPVARIANT` vazio, como era
    /// antes, só funciona em alguns decodificadores e deixava o vídeo parado depois de poucos loops).
    /// Se o rebobinar falhar, reabre o arquivo do zero.
    fn rewind(&self) {
        self.loops.set(self.loops.get() + 1);
        tracing::debug!(video = %self.name, loop_count = self.loops.get(), "Vídeo voltou ao começo");
        let zero = PROPVARIANT::from(0i64);
        let ok = unsafe { self.reader.borrow().SetCurrentPosition(&GUID::zeroed(), &zero).is_ok() };
        if !ok {
            self.reopen();
        }
    }

    /// Recria o leitor do arquivo (último recurso: libera o decodificador que travou).
    fn reopen(&self) {
        match open_reader(&self._path) {
            Ok(r) => {
                info!(video = %self.name, "Leitor de vídeo reaberto");
                *self.reader.borrow_mut() = r;
                self.last_decoded.set(Instant::now());
            }
            Err(e) => error!(video = %self.name, error = %e, "Não consegui reabrir o vídeo"),
        }
    }

    /// Inicializa a reprodução de vídeo acelerada por GPU a partir de arquivo de mídia.
    pub fn from_file(device: &ID3D11Device, path: PathBuf) -> Result<Self> {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("video")
            .to_string();

        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;

            let reader = open_reader(&path)?;

            // Lê as dimensões e taxa de quadros (FPS) do fluxo de vídeo
            let current_type =
                reader.GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)?;
            let frame_size = current_type.GetUINT64(&MF_MT_FRAME_SIZE)?;
            let width = (frame_size >> 32) as u32;
            let height = (frame_size & 0xFFFFFFFF) as u32;

            let fps = match current_type.GetUINT64(&MF_MT_FRAME_RATE) {
                Ok(rate) => {
                    let num = (rate >> 32) as f64;
                    let den = (rate & 0xFFFFFFFF) as f64;
                    if den > 0.0 {
                        (num / den).clamp(1.0, 120.0)
                    } else {
                        30.0
                    }
                }
                Err(_) => 30.0,
            };

            let frame_interval = Duration::from_secs_f64(1.0 / fps);

            // Duração para o loop sem corte: o fim se mistura com o começo durante `CROSSFADE`.
            let duration = reader
                .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
                .ok()
                .and_then(|v| u64::try_from(&v).ok())
                .and_then(|d| i64::try_from(d).ok())
                .unwrap_or(0);
            let fade_len = if duration >= MIN_LOOP_FOR_FADE { CROSSFADE.min(duration / 4) } else { 0 };

            // Cria textura D3D11 dinâmica para receber os quadros decodificados
            let tex_desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                MiscFlags: 0,
            };

            let make_tex = || -> Result<(ID3D11Texture2D, ID3D11ShaderResourceView)> {
                let mut texture = None;
                device.CreateTexture2D(&tex_desc, None, Some(&mut texture))?;
                let texture = texture.unwrap();
                let mut srv = None;
                device.CreateShaderResourceView(&texture, None, Some(&mut srv))?;
                Ok((texture, srv.unwrap()))
            };
            let (tex0, srv0) = make_tex()?;
            let (tex1, srv1) = make_tex()?;

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
            let sampler = sampler.unwrap();

            let cb_desc = D3D11_BUFFER_DESC {
                ByteWidth: 16,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                ..Default::default()
            };
            let mut cbuffer = None;
            device.CreateBuffer(&cb_desc, None, Some(&mut cbuffer))?;
            let cbuffer = cbuffer.unwrap();

            // Compila shaders do pipeline de textura
            let vs_blob = compile_hlsl(VIDEO_HLSL, "video_vs", s!("vs_main"), s!("vs_5_0"))?;
            let ps_blob = compile_hlsl(VIDEO_HLSL, "video_ps", s!("ps_main"), s!("ps_5_0"))?;

            let mut vs = None;
            let mut ps = None;
            device.CreateVertexShader(blob_bytes(&vs_blob), None, Some(&mut vs))?;
            device.CreatePixelShader(blob_bytes(&ps_blob), None, Some(&mut ps))?;

            info!(
                name = %name,
                width = width,
                height = height,
                fps = format!("{:.1}", fps),
                "Wallpaper de vídeo inicializado com sucesso via Media Foundation"
            );

            Ok(Self {
                name,
                _path: path,
                width,
                height,
                frame_interval,
                last_frame_time: RefCell::new(Instant::now() - frame_interval),
                last_bgra: RefCell::new(Vec::new()),
                reader: RefCell::new(reader),
                last_decoded: Cell::new(Instant::now()),
                loops: Cell::new(0),
                textures: [tex0, tex1],
                srvs: [srv0, srv1],
                prim: Cell::new(0),
                secondary: RefCell::new(None),
                secondary_started: Cell::new(false),
                duration,
                fade_len,
                blend: Cell::new(0.0),
                sampler,
                cbuffer,
                vs: vs.unwrap(),
                ps: ps.unwrap(),
            })
        }
    }

    /// Lê um quadro de `reader` e o grava em `textures[idx]`. Só o quadro principal vira "último quadro" do snapshot.
    fn read_into(&self, reader: &IMFSourceReader, idx: usize, context: &ID3D11DeviceContext, keep: bool) -> Read {
        unsafe {
            let mut flags = 0u32;
            let mut timestamp = 0i64;
            let mut sample: Option<IMFSample> = None;
            let hr = reader.ReadSample(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                0,
                None,
                Some(&mut flags),
                Some(&mut timestamp),
                Some(&mut sample),
            );
            if hr.is_err() {
                return Read::Failed;
            }
            if (flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
                return Read::End;
            }
            let Some(sample) = sample else { return Read::Gap };

            if let Ok(buffer) = sample.ConvertToContiguousBuffer() {
                let mut ptr = std::ptr::null_mut();
                let mut max_len = 0u32;
                let mut cur_len = 0u32;
                if buffer.Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len)).is_ok() {
                    let row_pitch = (self.width * 4) as usize;
                    let total_bytes = row_pitch * (self.height as usize);
                    let src_slice = std::slice::from_raw_parts(ptr, cur_len as usize);
                    if keep && src_slice.len() >= total_bytes {
                        let mut saved = self.last_bgra.borrow_mut();
                        saved.clear();
                        saved.extend_from_slice(&src_slice[..total_bytes]);
                    }
                    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                    if context.Map(&self.textures[idx], 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped)).is_ok() {
                        let dst_ptr = mapped.pData as *mut u8;
                        for y in 0..self.height {
                            let src_offset = (y as usize) * row_pitch;
                            let dst_offset = (y as usize) * (mapped.RowPitch as usize);
                            if src_offset + row_pitch <= src_slice.len() {
                                std::ptr::copy_nonoverlapping(src_slice.as_ptr().add(src_offset), dst_ptr.add(dst_offset), row_pitch);
                            }
                        }
                        context.Unmap(&self.textures[idx], 0);
                    }
                    let _ = buffer.Unlock();
                }
            }
            Read::Frame(timestamp)
        }
    }

    /// Atualiza o próximo quadro de vídeo se o intervalo de tempo tiver decorrido.
    fn advance_frame(&self, context: &ID3D11DeviceContext) {
        let now = Instant::now();
        {
            let mut last_time = self.last_frame_time.borrow_mut();
            if now.duration_since(*last_time) < self.frame_interval {
                return;
            }
            *last_time = now;
        }

        let read = {
            let reader = self.reader.borrow();
            self.read_into(&reader, self.prim.get(), context, true)
        };
        match read {
            Read::Failed => {
                // Leitor em estado ruim (acontece depois de seeks em alguns decodificadores): recomeça do zero.
                self.drop_secondary();
                self.reopen();
            }
            Read::End => self.finish_loop(),
            Read::Gap => {
                // Sem quadro agora (gap do fluxo, troca de tipo): não é o fim. Se isto durar, o vigia reabre.
                if self.last_decoded.get().elapsed() > STALL_LIMIT {
                    self.reopen();
                }
            }
            Read::Frame(ts) => {
                self.last_decoded.set(now);
                self.update_overlap(ts, context);
            }
        }
    }

    fn drop_secondary(&self) {
        *self.secondary.borrow_mut() = None;
        self.secondary_started.set(false);
        self.blend.set(0.0);
    }

    /// No fim do vídeo, a cópia do começo (se já estava rodando) vira o vídeo principal sem corte; senão, rebobina.
    fn finish_loop(&self) {
        let sec = self.secondary.borrow_mut().take();
        match sec {
            Some(next) if self.secondary_started.get() => {
                self.loops.set(self.loops.get() + 1);
                tracing::debug!(video = %self.name, loop_count = self.loops.get(), "Loop sem corte (fade)");
                *self.reader.borrow_mut() = next;
                self.prim.set(1 - self.prim.get());
                self.secondary_started.set(false);
                self.blend.set(0.0);
            }
            _ => {
                self.drop_secondary();
                self.rewind();
            }
        }
    }

    /// Nos últimos instantes do vídeo, roda uma cópia do começo e mistura as duas (loop que não parece loop).
    fn update_overlap(&self, ts: i64, context: &ID3D11DeviceContext) {
        if self.fade_len == 0 {
            return;
        }
        let start = self.duration - self.fade_len;
        if ts >= start - WARMUP
            && self.secondary.borrow().is_none()
            && let Ok(r) = open_reader(&self._path)
        {
            *self.secondary.borrow_mut() = Some(r);
            self.secondary_started.set(false);
        }
        if ts < start {
            return;
        }
        let sec_idx = 1 - self.prim.get();
        let res = self.secondary.borrow().as_ref().map(|r| self.read_into(r, sec_idx, context, false));
        match res {
            Some(Read::Frame(_)) => {
                self.secondary_started.set(true);
                let a = ((ts - start) as f32 / self.fade_len as f32).clamp(0.0, 1.0);
                self.blend.set(a * a * (3.0 - 2.0 * a));
            }
            Some(Read::Gap) | None => {}
            Some(_) => self.drop_secondary(),   // a cópia falhou: o fim cai no loop com corte
        }
    }
}

impl Wallpaper for VideoWallpaper {
    fn name(&self) -> &str {
        &self.name
    }

    fn supports_fade(&self) -> bool {
        true
    }

    fn snapshot_rgba(&self) -> Option<(u32, u32, Vec<u8>)> {
        let bgra = self.last_bgra.borrow();
        if bgra.len() < (self.width * self.height * 4) as usize {
            return None;
        }
        let mut rgba = bgra.clone();
        for px in rgba.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
            px[3] = 255;
        }
        Some((self.width, self.height, rgba))
    }

    fn render(
        &self,
        context: &ID3D11DeviceContext,
        surface: &RenderSurface,
        constants: ShaderConstants,
    ) -> Result<()> {
        self.advance_frame(context);

        unsafe {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            if context.Map(&self.cbuffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped)).is_ok() {
                let mut fade = constants.fade;
                fade[1] = self.blend.get();
                *(mapped.pData as *mut [f32; 4]) = fade;
                context.Unmap(&self.cbuffer, 0);
            }

            let viewport = D3D11_VIEWPORT {
                Width: surface.width as f32,
                Height: surface.height as f32,
                MaxDepth: 1.0,
                ..Default::default()
            };

            context.OMSetRenderTargets(Some(&[Some(surface.rtv.clone())]), None);
            context.RSSetViewports(Some(&[viewport]));
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vs, None);
            context.PSSetShader(&self.ps, None);
            context.PSSetConstantBuffers(0, Some(&[Some(self.cbuffer.clone())]));
            let prim = self.prim.get();
            context.PSSetShaderResources(0, Some(&[Some(self.srvs[prim].clone()), Some(self.srvs[1 - prim].clone())]));
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));

            context.Draw(3, 0);
            surface
                .swap_chain
                .Present(0, windows::Win32::Graphics::Dxgi::DXGI_PRESENT(0))
                .ok()
        }
    }

    fn check_reload(&mut self, _device: &ID3D11Device) -> Result<bool> {
        Ok(false)
    }
}

fn compile_hlsl(code: &str, name: &str, entry: PCSTR, target: PCSTR) -> Result<ID3DBlob> {
    let mut blob = None;
    let mut errors = None;
    let source_name = std::ffi::CString::new(name).unwrap_or_default();

    let result = unsafe {
        D3DCompile(
            code.as_ptr() as *const _,
            code.len(),
            PCSTR(source_name.as_ptr() as *const _),
            None,
            None,
            entry,
            target,
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut blob,
            Some(&mut errors),
        )
    };
    if let Err(e) = result {
        let msg = errors
            .map(|b| String::from_utf8_lossy(blob_bytes(&b)).into_owned())
            .unwrap_or_default();
        error!(shader = %name, error = %msg, "Falha na compilação do shader HLSL de vídeo");
        return Err(Error::new(
            e.code(),
            format!("falha ao compilar shader de vídeo {name}: {msg}"),
        ));
    }
    blob.ok_or_else(|| Error::new(E_FAIL, "D3DCompile não retornou código"))
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize())
    }
}
