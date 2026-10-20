//! Gerenciador de contexto Direct3D 11 e recuperação de Device Loss.

use super::surface::RenderSurface;
use crate::config::PresentMode;
use tracing::{info, warn};
use windows::{
    Win32::Foundation::*, Win32::Graphics::Direct3D::*, Win32::Graphics::Direct3D11::*,
    Win32::Graphics::DirectComposition::*, Win32::Graphics::Dxgi::*, core::*,
};

pub struct GpuContext {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub factory: IDXGIFactory2,
    pub dcomp: Option<IDCompositionDevice>,
    pub mode: PresentMode,
}

impl GpuContext {
    pub fn new(mode: PresentMode) -> Result<Self> {
        let (device, context, factory, dcomp) = Self::init_hardware(mode)?;

        Ok(Self {
            device,
            context,
            factory,
            dcomp,
            mode,
        })
    }

    /// Cria uma nova superfície para a janela do monitor.
    pub fn create_surface(&self, hwnd: HWND, width: u32, height: u32) -> Result<RenderSurface> {
        RenderSurface::new(
            &self.device,
            &self.factory,
            self.dcomp.as_ref(),
            hwnd,
            self.mode,
            width,
            height,
        )
    }

    /// Verifica se o dispositivo gráfico foi perdido (ex: atualização de driver ou suspensão).
    pub fn is_device_lost(&self, result: &Result<()>) -> bool {
        if let Err(e) = result {
            let code = e.code();
            code == DXGI_ERROR_DEVICE_REMOVED
                || code == DXGI_ERROR_DEVICE_RESET
                || code == DXGI_ERROR_DRIVER_INTERNAL_ERROR
        } else {
            unsafe {
                let reason = self.device.GetDeviceRemovedReason();
                reason.is_err()
            }
        }
    }

    /// Reinicializa a GPU após uma perda de dispositivo (Device Lost).
    pub fn recover(&mut self) -> Result<()> {
        warn!("Iniciando recuperação de GPU (Device Recovery)...");
        let (device, context, factory, dcomp) = Self::init_hardware(self.mode)?;

        self.device = device;
        self.context = context;
        self.factory = factory;
        self.dcomp = dcomp;

        info!("GPU reinicializada com sucesso!");
        Ok(())
    }

    fn init_hardware(
        mode: PresentMode,
    ) -> Result<(
        ID3D11Device,
        ID3D11DeviceContext,
        IDXGIFactory2,
        Option<IDCompositionDevice>,
    )> {
        unsafe {
            let mut device = None;
            let mut context = None;

            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;

            let device = device.ok_or_else(|| Error::new(E_FAIL, "Falha ao criar D3D11Device"))?;
            let context =
                context.ok_or_else(|| Error::new(E_FAIL, "Falha ao criar D3D11DeviceContext"))?;

            let dxgi_device: IDXGIDevice = device.cast()?;
            let adapter = dxgi_device.GetAdapter()?;
            let desc = adapter.GetDesc()?;
            let name_len = desc
                .Description
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(desc.Description.len());
            let gpu_name = String::from_utf16_lossy(&desc.Description[..name_len]);
            info!(gpu = %gpu_name, "Adaptador gráfico inicializado");

            let factory: IDXGIFactory2 = adapter.GetParent()?;

            let dcomp = match mode {
                PresentMode::Composition => Some(
                    DCompositionCreateDevice::<_, IDCompositionDevice>(&dxgi_device)?,
                ),
                PresentMode::Hwnd => None,
            };

            Ok((device, context, factory, dcomp))
        }
    }
}
