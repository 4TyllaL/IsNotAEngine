//! Espelha um quadro do wallpaper como imagem da tela de bloqueio.
//!
//! A tela de bloqueio roda fora da sessão do desktop, então só aceita uma imagem estática
//! (`LockScreen.SetImageFileAsync`). A API não devolve o modo anterior (ex.: Windows Spotlight),
//! por isso a imagem em uso é copiada uma única vez para `original_lockscreen.img`, e o recurso
//! fica desligado por padrão. Ao encerrar a engine a imagem NÃO é revertida.

use std::path::{Path, PathBuf};
use tracing::{info, warn};
use windows::Storage::Streams::DataReader;
use windows::Storage::StorageFile;
use windows::System::UserProfile::LockScreen;
use windows::core::HSTRING;

/// Copia a imagem atual da tela de bloqueio, se ainda não houver backup.
fn backup_current(dest: &Path) -> windows::core::Result<()> {
    if dest.exists() {
        return Ok(());
    }
    let stream = LockScreen::GetImageStream()?;
    let size = stream.Size()? as u32;
    if size == 0 {
        return Ok(());
    }
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
    reader.LoadAsync(size)?.join()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf)?;
    let _ = std::fs::write(dest, buf);
    Ok(())
}

fn apply_blocking(image: &Path, backup: &Path) -> windows::core::Result<()> {
    let _ = backup_current(backup).map_err(|e| warn!(error = %e, "Sem backup da tela de bloqueio atual"));
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(image.as_os_str()))?.join()?;
    LockScreen::SetImageFileAsync(&file)?.join()
}

/// Define `png` como imagem da tela de bloqueio, em segundo plano (as chamadas WinRT
/// bloqueiam, então não rodam na thread do motor). `dir` é a pasta de dados do motor.
pub fn set_image_async(dir: PathBuf, png: Vec<u8>) {
    std::thread::spawn(move || {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let image = dir.join("lockscreen.png");
        if let Err(e) = std::fs::write(&image, png) {
            warn!(error = %e, "Falha ao gravar a imagem da tela de bloqueio");
            return;
        }
        match apply_blocking(&image, &dir.join("original_lockscreen.img")) {
            Ok(()) => info!("Tela de bloqueio atualizada com o wallpaper atual"),
            Err(e) => warn!(error = %e, "Não foi possível definir a tela de bloqueio"),
        }
    });
}
