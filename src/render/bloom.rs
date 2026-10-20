//! Bloom em pós-processamento para os wallpapers de shader.
//!
//! A cena é renderizada numa textura RGBA16F; dela sai um bright-pass com joelho suave,
//! uma cadeia de downsamples (dual filter) e um upsample aditivo que soma todos os níveis.
//! O composite final soma o bloom à cena, comprime os realces e aplica dithering antes
//! de escrever no back buffer. Os alvos são criados por tamanho de superfície (um por monitor).

use std::cell::RefCell;
use std::collections::HashMap;

use windows::{
    Win32::Graphics::Direct3D::*, Win32::Graphics::Direct3D11::*,
    Win32::Graphics::Dxgi::Common::*, core::*,
};

use super::pipeline::get_or_compile_bytecode;

const LEVELS: usize = 6;

const BLOOM_HLSL: &str = r#"
cbuffer Bloom : register(b0)
{
    float2 src_texel;
    float2 inv_dst;
    float  threshold;
    float  knee;
    float  intensity;
    float  time;
};

Texture2D    src   : register(t0);
Texture2D    bloom : register(t1);
SamplerState samp  : register(s0);

float4 vs_main(uint id : SV_VertexID) : SV_Position
{
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
}

float luma(float3 c) { return dot(c, float3(0.2126, 0.7152, 0.0722)); }

// Média de Karis: evita que pixels isolados muito claros (grão, estrelas) pisquem.
float3 karis(float3 c) { return c / (1.0 + luma(c)); }

float3 down4(float2 uv)
{
    float2 o = src_texel;
    float3 a = src.Sample(samp, uv + float2(-o.x, -o.y)).rgb;
    float3 b = src.Sample(samp, uv + float2( o.x, -o.y)).rgb;
    float3 c = src.Sample(samp, uv + float2(-o.x,  o.y)).rgb;
    float3 d = src.Sample(samp, uv + float2( o.x,  o.y)).rgb;
    float3 m = src.Sample(samp, uv).rgb;
    return (a + b + c + d) * 0.125 + m * 0.5;
}

float4 ps_prefilter(float4 pos : SV_Position) : SV_Target
{
    float2 uv = pos.xy * inv_dst;
    float2 o = src_texel;
    float3 a = src.Sample(samp, uv + float2(-o.x, -o.y)).rgb;
    float3 b = src.Sample(samp, uv + float2( o.x, -o.y)).rgb;
    float3 c = src.Sample(samp, uv + float2(-o.x,  o.y)).rgb;
    float3 d = src.Sample(samp, uv + float2( o.x,  o.y)).rgb;
    float wa = 1.0 / (1.0 + luma(a)), wb = 1.0 / (1.0 + luma(b));
    float wc = 1.0 / (1.0 + luma(c)), wd = 1.0 / (1.0 + luma(d));
    float3 col = (a * wa + b * wb + c * wc + d * wd) / (wa + wb + wc + wd);

    // Limiar com joelho suave (curva quadrática), preservando a cor.
    float br = max(col.r, max(col.g, col.b));
    float rq = clamp(br - threshold + knee, 0.0, 2.0 * knee);
    rq = rq * rq / (4.0 * knee + 1e-5);
    float w = max(rq, br - threshold) / max(br, 1e-5);
    return float4(col * w, 1.0);
}

float4 ps_down(float4 pos : SV_Position) : SV_Target
{
    return float4(down4(pos.xy * inv_dst), 1.0);
}

float4 ps_up(float4 pos : SV_Position) : SV_Target
{
    float2 uv = pos.xy * inv_dst;
    float2 o = src_texel;
    float3 s = 0.0;
    s += src.Sample(samp, uv + float2(-2.0 * o.x, 0.0)).rgb;
    s += src.Sample(samp, uv + float2( 2.0 * o.x, 0.0)).rgb;
    s += src.Sample(samp, uv + float2(0.0, -2.0 * o.y)).rgb;
    s += src.Sample(samp, uv + float2(0.0,  2.0 * o.y)).rgb;
    s += src.Sample(samp, uv + float2(-o.x, -o.y)).rgb * 2.0;
    s += src.Sample(samp, uv + float2( o.x, -o.y)).rgb * 2.0;
    s += src.Sample(samp, uv + float2(-o.x,  o.y)).rgb * 2.0;
    s += src.Sample(samp, uv + float2( o.x,  o.y)).rgb * 2.0;
    return float4(s / 12.0, 1.0);
}

float hash12(float2 p)
{
    float3 p3 = frac(float3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return frac((p3.x + p3.y) * p3.z);
}

float4 ps_composite(float4 pos : SV_Position) : SV_Target
{
    float2 uv = pos.xy * inv_dst;
    float3 col = src.Sample(samp, uv).rgb;
    float3 glow = bloom.Sample(samp, uv).rgb;
    col += glow * intensity;

    // Ombro suave para que o bloom não estoure em branco chapado.
    float3 over = max(col - 0.85, 0.0);
    col = min(col, 0.85) + over / (1.0 + over / 0.15);

    float2 seed = pos.xy + frac(time * 0.53) * 311.0;
    float tpdf = hash12(seed) + hash12(seed + 17.7) - 1.0;
    return float4(saturate(col + tpdf / 255.0), 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BloomConstants {
    src_texel: [f32; 2],
    inv_dst: [f32; 2],
    threshold: f32,
    knee: f32,
    intensity: f32,
    time: f32,
}

/// Configuração de bloom de um wallpaper, lida da diretiva `// bloom: <intensidade> [limiar]`.
#[derive(Clone, Copy, Debug)]
pub struct BloomSettings {
    pub intensity: f32,
    pub threshold: f32,
}

impl Default for BloomSettings {
    fn default() -> Self {
        Self { intensity: 0.55, threshold: 0.62 }
    }
}

impl BloomSettings {
    /// Procura `// bloom: 0.8` ou `// bloom: 0.8 0.5` no código do shader.
    pub fn from_source(code: &str) -> Self {
        let mut s = Self::default();
        for line in code.lines().take(40) {
            let Some(rest) = line.trim().strip_prefix("//") else { continue };
            let Some(args) = rest.trim().strip_prefix("bloom:") else { continue };
            let mut it = args.split_whitespace().filter_map(|v| v.parse::<f32>().ok());
            if let Some(i) = it.next() {
                s.intensity = i.max(0.0);
            }
            if let Some(t) = it.next() {
                s.threshold = t.clamp(0.0, 4.0);
            }
            break;
        }
        s
    }

    pub fn enabled(&self) -> bool {
        self.intensity > 0.001
    }
}

struct Target {
    srv: ID3D11ShaderResourceView,
    rtv: ID3D11RenderTargetView,
    width: u32,
    height: u32,
}

struct Targets {
    scene: Target,
    levels: Vec<Target>,
}

pub struct Bloom {
    vs: ID3D11VertexShader,
    prefilter: ID3D11PixelShader,
    down: ID3D11PixelShader,
    up: ID3D11PixelShader,
    composite: ID3D11PixelShader,
    cb: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    additive: ID3D11BlendState,
    targets: RefCell<HashMap<(u32, u32), Targets>>,
}

fn create_target(device: &ID3D11Device, w: u32, h: u32, format: DXGI_FORMAT) -> Result<Target> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        ..Default::default()
    };
    unsafe {
        let mut tex = None;
        device.CreateTexture2D(&desc, None, Some(&mut tex))?;
        let tex = tex.unwrap();
        let mut srv = None;
        device.CreateShaderResourceView(&tex, None, Some(&mut srv))?;
        let mut rtv = None;
        device.CreateRenderTargetView(&tex, None, Some(&mut rtv))?;
        Ok(Target { srv: srv.unwrap(), rtv: rtv.unwrap(), width: w, height: h })
    }
}

impl Bloom {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        unsafe {
            let ps = |entry: PCSTR| -> Result<ID3D11PixelShader> {
                let bytes = get_or_compile_bytecode(BLOOM_HLSL, "bloom", entry, s!("ps_5_0"))?;
                let mut out = None;
                device.CreatePixelShader(&bytes, None, Some(&mut out))?;
                Ok(out.unwrap())
            };
            let vs_bytes = get_or_compile_bytecode(BLOOM_HLSL, "bloom", s!("vs_main"), s!("vs_5_0"))?;
            let mut vs = None;
            device.CreateVertexShader(&vs_bytes, None, Some(&mut vs))?;

            let cb_desc = D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of::<BloomConstants>() as u32,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                ..Default::default()
            };
            let mut cb = None;
            device.CreateBuffer(&cb_desc, None, Some(&mut cb))?;

            let sampler_desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            let mut sampler = None;
            device.CreateSamplerState(&sampler_desc, Some(&mut sampler))?;

            let mut blend_desc = D3D11_BLEND_DESC::default();
            blend_desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D11_BLEND_ONE,
                DestBlend: D3D11_BLEND_ONE,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: D3D11_BLEND_ZERO,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            let mut additive = None;
            device.CreateBlendState(&blend_desc, Some(&mut additive))?;

            Ok(Self {
                vs: vs.unwrap(),
                prefilter: ps(s!("ps_prefilter"))?,
                down: ps(s!("ps_down"))?,
                up: ps(s!("ps_up"))?,
                composite: ps(s!("ps_composite"))?,
                cb: cb.unwrap(),
                sampler: sampler.unwrap(),
                additive: additive.unwrap(),
                targets: RefCell::new(HashMap::new()),
            })
        }
    }

    /// Garante os alvos para o tamanho dado e devolve a RTV onde a cena deve ser desenhada.
    pub fn scene_rtv(&self, context: &ID3D11DeviceContext, w: u32, h: u32) -> Result<ID3D11RenderTargetView> {
        let mut map = self.targets.borrow_mut();
        if let std::collections::hash_map::Entry::Vacant(e) = map.entry((w, h)) {
            let device = unsafe { context.GetDevice()? };
            let scene = create_target(&device, w, h, DXGI_FORMAT_R16G16B16A16_FLOAT)?;
            let mut levels = Vec::with_capacity(LEVELS);
            let (mut lw, mut lh) = (w, h);
            for _ in 0..LEVELS {
                lw = (lw / 2).max(1);
                lh = (lh / 2).max(1);
                levels.push(create_target(&device, lw, lh, DXGI_FORMAT_R11G11B10_FLOAT)?);
            }
            e.insert(Targets { scene, levels });
        }
        Ok(map[&(w, h)].scene.rtv.clone())
    }

    #[allow(clippy::too_many_arguments)]
    fn pass(
        &self,
        context: &ID3D11DeviceContext,
        ps: &ID3D11PixelShader,
        src: &Target,
        dst_rtv: &ID3D11RenderTargetView,
        dst_w: u32,
        dst_h: u32,
        consts: BloomConstants,
        extra: Option<&ID3D11ShaderResourceView>,
    ) -> Result<()> {
        unsafe {
            // Desvincula SRVs antes de trocar de alvo para evitar hazards leitura/escrita.
            context.PSSetShaderResources(0, Some(&[None, None]));
            let mut c = consts;
            c.src_texel = [1.0 / src.width as f32, 1.0 / src.height as f32];
            c.inv_dst = [1.0 / dst_w as f32, 1.0 / dst_h as f32];
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&self.cb, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))?;
            (mapped.pData as *mut BloomConstants).write(c);
            context.Unmap(&self.cb, 0);

            context.OMSetRenderTargets(Some(&[Some(dst_rtv.clone())]), None);
            context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                Width: dst_w as f32,
                Height: dst_h as f32,
                MaxDepth: 1.0,
                ..Default::default()
            }]));
            context.PSSetShader(ps, None);
            context.PSSetShaderResources(0, Some(&[Some(src.srv.clone()), extra.cloned()]));
            context.Draw(3, 0);
        }
        Ok(())
    }

    /// Aplica o bloom sobre a cena já desenhada em `scene_rtv` e escreve o resultado em `out`.
    pub fn apply(
        &self,
        context: &ID3D11DeviceContext,
        out: &ID3D11RenderTargetView,
        w: u32,
        h: u32,
        settings: BloomSettings,
        time: f32,
    ) -> Result<()> {
        let map = self.targets.borrow();
        let t = &map[&(w, h)];
        let base = BloomConstants {
            threshold: settings.threshold,
            knee: settings.threshold * 0.5 + 1e-3,
            intensity: settings.intensity,
            time,
            ..Default::default()
        };

        unsafe {
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vs, None);
            context.PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            context.OMSetBlendState(None, None, 0xffff_ffff);
        }

        // Bright-pass + primeiro downsample, depois a cadeia de downsamples.
        let l = &t.levels;
        self.pass(context, &self.prefilter, &t.scene, &l[0].rtv, l[0].width, l[0].height, base, None)?;
        for i in 1..LEVELS {
            self.pass(context, &self.down, &l[i - 1], &l[i].rtv, l[i].width, l[i].height, base, None)?;
        }

        // Upsample aditivo: cada nível recebe a soma dos níveis menores.
        unsafe { context.OMSetBlendState(&self.additive, None, 0xffff_ffff) };
        for i in (1..LEVELS).rev() {
            self.pass(context, &self.up, &l[i], &l[i - 1].rtv, l[i - 1].width, l[i - 1].height, base, None)?;
        }
        unsafe { context.OMSetBlendState(None, None, 0xffff_ffff) };

        // Composite: cena (t0) + bloom (t1) no back buffer. A soma de LEVELS níveis é normalizada.
        let mut comp = base;
        comp.intensity = settings.intensity / LEVELS as f32 * 2.0;
        self.pass(context, &self.composite, &t.scene, out, w, h, comp, Some(&l[0].srv))?;
        unsafe { context.PSSetShaderResources(0, Some(&[None, None])) };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bloom_shaders_compile() {
        for entry in [s!("ps_prefilter"), s!("ps_down"), s!("ps_up"), s!("ps_composite")] {
            assert!(get_or_compile_bytecode(BLOOM_HLSL, "bloom", entry, s!("ps_5_0")).is_ok());
        }
        assert!(get_or_compile_bytecode(BLOOM_HLSL, "bloom", s!("vs_main"), s!("vs_5_0")).is_ok());
    }

    #[test]
    fn parses_directive() {
        let s = BloomSettings::from_source("// x
// bloom: 0.8 0.4
float a;");
        assert_eq!((s.intensity, s.threshold), (0.8, 0.4));
        assert!(!BloomSettings::from_source("// bloom: 0").enabled());
    }
}
