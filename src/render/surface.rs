//! Superfície de apresentação GPU para uma janela/monitor.

use crate::config::PresentMode;
use windows::{
    Win32::Foundation::*, Win32::Graphics::Direct3D11::*, Win32::Graphics::DirectComposition::*,
    Win32::Graphics::Dxgi::Common::*, Win32::Graphics::Dxgi::*, core::*,
};

pub struct RenderSurface {
    pub _hwnd: HWND,
    pub swap_chain: IDXGISwapChain1,
    pub rtv: ID3D11RenderTargetView,
    pub width: u32,
    pub height: u32,
    _target: Option<IDCompositionTarget>,
    _visual: Option<IDCompositionVisual>,
}

impl RenderSurface {
    pub fn new(
        device: &ID3D11Device,
        factory: &IDXGIFactory2,
        dcomp: Option<&IDCompositionDevice>,
        hwnd: HWND,
        mode: PresentMode,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            ..Default::default()
        };

        unsafe {
            let (swap_chain, target, visual) = match (mode, dcomp) {
                (PresentMode::Composition, Some(dcomp)) => {
                    let sc = factory.CreateSwapChainForComposition(device, &desc, None)?;
                    let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
                    let visual = dcomp.CreateVisual()?;
                    visual.SetContent(&sc)?;
                    target.SetRoot(&visual)?;
                    dcomp.Commit()?;
                    (sc, Some(target), Some(visual))
                }
                _ => {
                    let sc = factory.CreateSwapChainForHwnd(device, hwnd, &desc, None, None)?;
                    (sc, None, None)
                }
            };

            let back_buffer: ID3D11Texture2D = swap_chain.GetBuffer(0)?;
            let mut rtv = None;
            device.CreateRenderTargetView(&back_buffer, None, Some(&mut rtv))?;

            Ok(Self {
                _hwnd: hwnd,
                swap_chain,
                rtv: rtv.unwrap(),
                width,
                height,
                _target: target,
                _visual: visual,
            })
        }
    }
}
