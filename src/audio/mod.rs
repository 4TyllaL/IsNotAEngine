//! Captura de áudio nativa via WASAPI Loopback do Windows e análise de frequências em tempo real.
#![allow(unsafe_op_in_unsafe_fn)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tracing::{debug, info, warn};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

const NUM_SPECTRUM_BANDS: usize = 16;

/// Estado instantâneo de áudio disponibilizado para os shaders GPU.
#[derive(Debug, Clone, Copy)]
pub struct AudioSnapshot {
    /// x: bass (graves), y: mid (médios), z: treble (agudos), w: volume global (RMS)
    pub reactive: [f32; 4],
    /// 16 bandas de frequência do espectro normalizadas (0.0 .. 1.0)
    pub spectrum: [f32; NUM_SPECTRUM_BANDS],
}

impl Default for AudioSnapshot {
    fn default() -> Self {
        Self {
            reactive: [0.0; 4],
            spectrum: [0.0; NUM_SPECTRUM_BANDS],
        }
    }
}

/// Gerenciador de captura e análise de áudio WASAPI.
pub struct AudioEngine {
    running: Arc<AtomicBool>,
    state: Arc<Mutex<AudioSnapshot>>,
    worker: Option<JoinHandle<()>>,
}

impl AudioEngine {
    /// Inicializa a captura WASAPI Loopback em segundo plano.
    pub fn new() -> Self {
        let state = Arc::new(Mutex::new(AudioSnapshot::default()));
        let (running, worker) = Self::spawn(&state);
        Self {
            running,
            state,
            worker,
        }
    }

    fn spawn(state: &Arc<Mutex<AudioSnapshot>>) -> (Arc<AtomicBool>, Option<JoinHandle<()>>) {
        let running = Arc::new(AtomicBool::new(true));
        let (r_flag, s_ref) = (running.clone(), state.clone());
        let worker = thread::Builder::new()
            .name("IsNotAEngine-WASAPI".to_string())
            .spawn(move || {
                run_wasapi_loop(r_flag, s_ref);
            })
            .ok();
        (running, worker)
    }

    /// Para a captura (a thread acorda a cada ~10 ms enquanto roda) e zera a leitura.
    pub fn suspend(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
        if let Ok(mut s) = self.state.lock() {
            *s = AudioSnapshot::default();
        }
    }

    /// Retoma a captura depois de [`Self::suspend`]; não faz nada se já está rodando.
    pub fn resume(&mut self) {
        if self.worker.is_none() {
            (self.running, self.worker) = Self::spawn(&self.state);
        }
    }

    /// Obtém a leitura mais recente das frequências de áudio para o frame atual.
    pub fn snapshot(&self) -> AudioSnapshot {
        self.state.lock().map(|g| *g).unwrap_or_default()
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

fn run_wasapi_loop(running: Arc<AtomicBool>, state: Arc<Mutex<AudioSnapshot>>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    info!("Submódulo WASAPI Loopback iniciado");

    let mut smoothed_bands = [0.0f32; NUM_SPECTRUM_BANDS];
    let mut smoothed_bass = 0.0f32;
    let mut smoothed_mid = 0.0f32;
    let mut smoothed_treble = 0.0f32;
    let mut smoothed_vol = 0.0f32;

    while running.load(Ordering::SeqCst) {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            capture_session(
                &running,
                &state,
                &mut smoothed_bands,
                &mut smoothed_bass,
                &mut smoothed_mid,
                &mut smoothed_treble,
                &mut smoothed_vol,
            )
        }));

        if let Err(e) = res {
            warn!("Sessão WASAPI interrompida: {:?}. Tentando reconectar...", e);
        }

        if running.load(Ordering::SeqCst) {
            // Decaimento suave quando em silêncio ou reconectando
            decay_state(
                &state,
                &mut smoothed_bands,
                &mut smoothed_bass,
                &mut smoothed_mid,
                &mut smoothed_treble,
                &mut smoothed_vol,
            );
            thread::sleep(Duration::from_millis(500));
        }
    }

    unsafe {
        CoUninitialize();
    }
    info!("Submódulo WASAPI Loopback finalizado");
}

fn capture_session(
    running: &Arc<AtomicBool>,
    state: &Arc<Mutex<AudioSnapshot>>,
    smoothed_bands: &mut [f32; NUM_SPECTRUM_BANDS],
    smoothed_bass: &mut f32,
    smoothed_mid: &mut f32,
    smoothed_treble: &mut f32,
    smoothed_vol: &mut f32,
) {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
                Ok(e) => e,
                Err(err) => {
                    warn!(error = %err, "Falha ao instanciar MMDeviceEnumerator");
                    return;
                }
            };

        let device = match enumerator.GetDefaultAudioEndpoint(eRender, eConsole) {
            Ok(d) => d,
            Err(err) => {
                debug!(error = %err, "Nenhum endpoint de áudio padrão ativo");
                return;
            }
        };

        let audio_client: IAudioClient = match device.Activate(CLSCTX_ALL, None) {
            Ok(c) => c,
            Err(err) => {
                warn!(error = %err, "Falha ao ativar IAudioClient");
                return;
            }
        };

        let pwfx = match audio_client.GetMixFormat() {
            Ok(f) => f,
            Err(err) => {
                warn!(error = %err, "Falha ao obter formato de áudio (GetMixFormat)");
                return;
            }
        };

        let wfx = *pwfx;
        let channels = wfx.nChannels as usize;
        let sample_rate = wfx.nSamplesPerSec as f32;
        let is_float = wfx.wBitsPerSample == 32;

        let bits = wfx.wBitsPerSample;
        debug!(channels, sample_rate, bits, "Inicializando WASAPI Loopback");

        // AUDCLNT_STREAMFLAGS_LOOPBACK permite capturar a saída master sem drivers
        let init_res = audio_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            10_000_000, // 1 segundo de buffer
            0,
            pwfx,
            None,
        );

        CoTaskMemFree(Some(pwfx as *const _ as *const std::ffi::c_void));

        if let Err(err) = init_res {
            warn!(error = %err, "Falha ao inicializar stream loopback");
            return;
        }

        let capture_client: IAudioCaptureClient = match audio_client.GetService() {
            Ok(cc) => cc,
            Err(err) => {
                warn!(error = %err, "Falha ao obter IAudioCaptureClient");
                return;
            }
        };

        if let Err(err) = audio_client.Start() {
            warn!(error = %err, "Falha ao iniciar IAudioClient");
            return;
        }

        info!("WASAPI Loopback capturando áudio com sucesso");

        let mut sample_buffer = Vec::<f32>::with_capacity(2048);

        while running.load(Ordering::SeqCst) {
            let mut packet_size = match capture_client.GetNextPacketSize() {
                Ok(s) => s,
                Err(_) => break,
            };

            if packet_size == 0 {
                // Silêncio / sem pacotes no momento: decai suavemente
                decay_state(
                    state,
                    smoothed_bands,
                    smoothed_bass,
                    smoothed_mid,
                    smoothed_treble,
                    smoothed_vol,
                );
                thread::sleep(Duration::from_millis(10));
                continue;
            }

            while packet_size > 0 {
                let mut data_ptr = std::ptr::null_mut();
                let mut num_frames = 0u32;
                let mut flags = 0u32;

                if capture_client
                    .GetBuffer(&mut data_ptr, &mut num_frames, &mut flags, None, None)
                    .is_ok()
                {
                    if (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) == 0 && !data_ptr.is_null() {
                        let total_samples = (num_frames as usize) * channels;
                        sample_buffer.clear();

                        if is_float {
                            let slice = std::slice::from_raw_parts(data_ptr as *const f32, total_samples);
                            // Converte canais estéreo/surround para mono
                            for frame in slice.chunks(channels) {
                                let sum: f32 = frame.iter().sum();
                                sample_buffer.push(sum / (channels as f32));
                            }
                        } else {
                            let slice = std::slice::from_raw_parts(data_ptr as *const i16, total_samples);
                            for frame in slice.chunks(channels) {
                                let sum: f32 = frame.iter().map(|&s| s as f32 / 32768.0).sum();
                                sample_buffer.push(sum / (channels as f32));
                            }
                        }

                        if !sample_buffer.is_empty() {
                            analyze_samples(
                                &sample_buffer,
                                sample_rate,
                                state,
                                smoothed_bands,
                                smoothed_bass,
                                smoothed_mid,
                                smoothed_treble,
                                smoothed_vol,
                            );
                        }
                    }

                    let _ = capture_client.ReleaseBuffer(num_frames);
                }

                match capture_client.GetNextPacketSize() {
                    Ok(s) => packet_size = s,
                    Err(_) => break,
                }
            }

            thread::sleep(Duration::from_millis(8));
        }

        let _ = audio_client.Stop();
    }
}

/// Analisa as amostras de áudio mono calculando bandas logarítmicas e RMS.
#[allow(clippy::too_many_arguments)]
fn analyze_samples(
    samples: &[f32],
    sample_rate: f32,
    state: &Arc<Mutex<AudioSnapshot>>,
    smoothed_bands: &mut [f32; NUM_SPECTRUM_BANDS],
    smoothed_bass: &mut f32,
    smoothed_mid: &mut f32,
    smoothed_treble: &mut f32,
    smoothed_vol: &mut f32,
) {
    let n = samples.len();
    if n == 0 {
        return;
    }

    // 1. RMS / Volume Global
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    let raw_rms = (sum_sq / n as f32).sqrt().min(1.0);
    // Ganho perceptivo
    let current_vol = (raw_rms * 2.8).min(1.0);

    // 2. Banco de filtros por DFT simplificada para as 16 bandas (20Hz a 18kHz em escala log)
    let min_freq = 25.0f32;
    let max_freq = 16000.0f32;
    let log_min = min_freq.ln();
    let log_max = max_freq.ln();

    let mut current_bands = [0.0f32; NUM_SPECTRUM_BANDS];

    for (i, band) in current_bands.iter_mut().enumerate() {
        let frac = (i as f32 + 0.5) / NUM_SPECTRUM_BANDS as f32;
        let center_freq = (log_min + frac * (log_max - log_min)).exp();

        // Goertzel / DFT simplificada na frequência central
        let omega = 2.0 * std::f32::consts::PI * center_freq / sample_rate;
        let mut real = 0.0f32;
        let mut imag = 0.0f32;

        let stride = (n / 128).max(1); // subamostragem se buffer for muito grande
        let mut count = 0;
        for (idx, &sample) in samples.iter().enumerate().step_by(stride) {
            let angle = omega * idx as f32;
            real += sample * angle.cos();
            imag += sample * angle.sin();
            count += 1;
        }

        if count > 0 {
            let magnitude = (real * real + imag * imag).sqrt() / count as f32;
            // Equalização perceptiva: frequências altas tendem a ter menor energia natural
            let eq_boost = 1.0 + (i as f32 * 0.25);
            *band = (magnitude * 8.0 * eq_boost).min(1.0);
        }
    }

    // 3. Agrupamento em Bass (0..3), Mid (4..10), Treble (11..15)
    let raw_bass = current_bands[0..4].iter().copied().fold(0.0f32, f32::max);
    let raw_mid = current_bands[4..11].iter().copied().fold(0.0f32, f32::max);
    let raw_treble = current_bands[11..16].iter().copied().fold(0.0f32, f32::max);

    // 4. Suavização temporal (Ataque rápido, queda suave / peak-decay visual)
    const ATTACK: f32 = 0.65;
    const DECAY: f32 = 0.82;

    for i in 0..NUM_SPECTRUM_BANDS {
        let target = current_bands[i];
        if target > smoothed_bands[i] {
            smoothed_bands[i] = smoothed_bands[i] * (1.0 - ATTACK) + target * ATTACK;
        } else {
            smoothed_bands[i] *= DECAY;
        }
    }

    if raw_bass > *smoothed_bass {
        *smoothed_bass = *smoothed_bass * (1.0 - ATTACK) + raw_bass * ATTACK;
    } else {
        *smoothed_bass *= DECAY;
    }

    if raw_mid > *smoothed_mid {
        *smoothed_mid = *smoothed_mid * (1.0 - ATTACK) + raw_mid * ATTACK;
    } else {
        *smoothed_mid *= DECAY;
    }

    if raw_treble > *smoothed_treble {
        *smoothed_treble = *smoothed_treble * (1.0 - ATTACK) + raw_treble * ATTACK;
    } else {
        *smoothed_treble *= DECAY;
    }

    if current_vol > *smoothed_vol {
        *smoothed_vol = *smoothed_vol * (1.0 - ATTACK) + current_vol * ATTACK;
    } else {
        *smoothed_vol *= DECAY;
    }

    // 5. Atualiza o estado atômico
    if let Ok(mut lock) = state.lock() {
        lock.reactive = [*smoothed_bass, *smoothed_mid, *smoothed_treble, *smoothed_vol];
        lock.spectrum = *smoothed_bands;
    }
}

fn decay_state(
    state: &Arc<Mutex<AudioSnapshot>>,
    smoothed_bands: &mut [f32; NUM_SPECTRUM_BANDS],
    smoothed_bass: &mut f32,
    smoothed_mid: &mut f32,
    smoothed_treble: &mut f32,
    smoothed_vol: &mut f32,
) {
    const DECAY: f32 = 0.85;
    for b in smoothed_bands.iter_mut() {
        *b *= DECAY;
    }
    *smoothed_bass *= DECAY;
    *smoothed_mid *= DECAY;
    *smoothed_treble *= DECAY;
    *smoothed_vol *= DECAY;

    if let Ok(mut lock) = state.lock() {
        lock.reactive = [*smoothed_bass, *smoothed_mid, *smoothed_treble, *smoothed_vol];
        lock.spectrum = *smoothed_bands;
    }
}

#[cfg(test)]
mod engine_tests {
    use super::*;

    #[test]
    fn suspend_stops_the_thread_and_resume_restarts_it() {
        let mut a = AudioEngine::new();
        a.suspend();
        assert!(a.worker.is_none());
        assert_eq!(a.snapshot().reactive, [0.0; 4]);
        a.suspend(); // repetir não quebra
        a.resume();
        assert!(a.worker.is_some());
        a.resume(); // já rodando: não cria outra thread
        a.suspend();
    }
}
