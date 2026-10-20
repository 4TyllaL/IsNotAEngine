//! Métricas do sistema para wallpapers web (`engine.system`): CPU total e por núcleo, memória,
//! disco, rede e tempo ligado. Uma thread amostra a cada segundo e publica um JSON versionado,
//! no mesmo padrão do `MediaWatcher`: o motor só reenvia quando a versão muda.
//!
//! Nada aqui sai da máquina: os valores vão apenas para a página do wallpaper (que roda com
//! `connect-src 'none'`). Não expõe nome do computador, usuário, processos nem endereços.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::{debug, info};
use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessorPerformanceInformation};
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::WindowsProgramming::SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION;
use windows::core::w;

const SAMPLE_EVERY: Duration = Duration::from_secs(1);
/// IF_TYPE_SOFTWARE_LOOPBACK e IfOperStatusUp (valores fixos da API do Windows).
const IF_TYPE_LOOPBACK: u32 = 24;
const IF_OPER_UP: i32 = 1;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct SystemInfo {
    /// Uso total de CPU, 0..100.
    pub cpu: f32,
    /// Uso por núcleo lógico, 0..100.
    pub cores: Vec<f32>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    /// Bytes por segundo recebidos/enviados (todas as interfaces ativas, exceto loopback).
    pub net_down: f64,
    pub net_up: f64,
    pub uptime_secs: u64,
}

/// Percentual de uso entre duas leituras `(ocioso, total)` dos contadores de tempo do Windows.
/// O tempo de kernel já inclui o ocioso, então `total = kernel + user`.
pub fn busy_percent(prev: (u64, u64), cur: (u64, u64)) -> f32 {
    let total = cur.1.saturating_sub(prev.1);
    if total == 0 {
        return 0.0;
    }
    let idle = cur.0.saturating_sub(prev.0).min(total);
    ((total - idle) as f64 / total as f64 * 100.0) as f32
}

/// Bytes por segundo a partir de dois valores acumulados; tolera contador que reinicia.
pub fn rate(prev: u64, cur: u64, secs: f64) -> f64 {
    if secs <= 0.0 || cur < prev { 0.0 } else { (cur - prev) as f64 / secs }
}

#[derive(Default)]
pub struct SystemSampler {
    prev_cores: Vec<(u64, u64)>,
    prev_net: Option<(Instant, u64, u64)>,
}

impl SystemSampler {
    pub fn sample(&mut self) -> SystemInfo {
        let mut info = SystemInfo::default();

        let cur = read_core_times();
        if self.prev_cores.len() == cur.len() {
            info.cores = self.prev_cores.iter().zip(&cur).map(|(p, c)| busy_percent(*p, *c)).collect();
        } else {
            info.cores = vec![0.0; cur.len()];
        }
        if !info.cores.is_empty() {
            info.cpu = info.cores.iter().sum::<f32>() / info.cores.len() as f32;
        }
        self.prev_cores = cur;

        let (used, total) = read_memory();
        info.mem_used = used;
        info.mem_total = total;
        let (used, total) = read_disk();
        info.disk_used = used;
        info.disk_total = total;
        info.uptime_secs = unsafe { GetTickCount64() } / 1000;

        let now = Instant::now();
        let (rx, tx) = read_net_counters();
        if let Some((t0, rx0, tx0)) = self.prev_net {
            let secs = now.duration_since(t0).as_secs_f64();
            info.net_down = rate(rx0, rx, secs);
            info.net_up = rate(tx0, tx, secs);
        }
        self.prev_net = Some((now, rx, tx));
        info
    }
}

/// `(ocioso, kernel + user)` por núcleo lógico, em unidades de 100 ns.
fn read_core_times() -> Vec<(u64, u64)> {
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(256);
    let mut buf = vec![SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION::default(); n];
    let bytes = (n * std::mem::size_of::<SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION>()) as u32;
    let mut returned = 0u32;
    // SAFETY: `buf` tem `bytes` bytes válidos e o tamanho informado bate com o buffer.
    let status = unsafe {
        NtQuerySystemInformation(SystemProcessorPerformanceInformation, buf.as_mut_ptr().cast(), bytes, &mut returned)
    };
    if status.is_err() {
        debug!(status = status.0, "NtQuerySystemInformation falhou");
        return Vec::new();
    }
    let count = (returned as usize / std::mem::size_of::<SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION>()).min(n);
    buf.iter()
        .take(count)
        .map(|c| (c.IdleTime.max(0) as u64, (c.KernelTime.max(0) as u64).saturating_add(c.UserTime.max(0) as u64)))
        .collect()
}

fn read_memory() -> (u64, u64) {
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: `m` é um MEMORYSTATUSEX válido com `dwLength` preenchido.
    if unsafe { GlobalMemoryStatusEx(&mut m) }.is_ok() {
        (m.ullTotalPhys.saturating_sub(m.ullAvailPhys), m.ullTotalPhys)
    } else {
        (0, 0)
    }
}

/// Uso do volume do sistema (C:). Só capacidade total e livre; nada sobre arquivos.
fn read_disk() -> (u64, u64) {
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: ponteiros para variáveis locais válidas durante a chamada.
    let ok = unsafe { GetDiskFreeSpaceExW(w!("C:\\"), Some(&mut avail), Some(&mut total), Some(&mut free)) };
    if ok.is_ok() { (total.saturating_sub(free), total) } else { (0, 0) }
}

/// Soma dos bytes recebidos/enviados de todas as interfaces ativas que não são loopback.
fn read_net_counters() -> (u64, u64) {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: GetIfTable2 aloca a tabela; ela é liberada com FreeMibTable abaixo.
    let err = unsafe { GetIfTable2(&mut table) };
    if err.is_err() || table.is_null() {
        return (0, 0);
    }
    let (mut rx, mut tx) = (0u64, 0u64);
    // SAFETY: `table` é válida até o FreeMibTable; `Table` é um array de `NumEntries` linhas.
    unsafe {
        let t = &*table;
        let rows = std::slice::from_raw_parts(t.Table.as_ptr(), t.NumEntries as usize);
        for r in rows {
            if r.Type != IF_TYPE_LOOPBACK && r.OperStatus.0 == IF_OPER_UP {
                rx = rx.saturating_add(r.InOctets);
                tx = tx.saturating_add(r.OutOctets);
            }
        }
        FreeMibTable(table.cast());
    }
    (rx, tx)
}

#[derive(Clone, Default)]
pub struct SystemWatcher {
    state: Arc<Mutex<(u64, String)>>,
    /// Modo desempenho: a thread não amostra enquanto estiver `true`.
    sleeping: Arc<std::sync::atomic::AtomicBool>,
}

impl SystemWatcher {
    pub fn start() -> Self {
        let watcher = Self::default();
        let state = watcher.state.clone();
        let sleeping = watcher.sleeping.clone();
        let spawned = std::thread::Builder::new().name("SystemWatcher".into()).spawn(move || {
            let mut sampler = SystemSampler::default();
            sampler.sample(); // primeira leitura só serve de base para as taxas
            let mut version = 0u64;
            loop {
                std::thread::sleep(SAMPLE_EVERY);
                if sleeping.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }
                let info = sampler.sample();
                if let Ok(json) = serde_json::to_string(&info) {
                    version += 1;
                    if let Ok(mut s) = state.lock() {
                        *s = (version, json);
                    }
                }
            }
        });
        if let Err(e) = spawned {
            info!(error = %e, "Não foi possível iniciar o SystemWatcher; engine.system ficará vazio");
        }
        watcher
    }

    pub fn set_sleeping(&self, sleeping: bool) {
        self.sleeping.store(sleeping, std::sync::atomic::Ordering::Relaxed);
    }

    /// `(versão, json)` mais recente; versão 0 = ainda sem amostra.
    pub fn snapshot(&self) -> (u64, String) {
        self.state.lock().map(|s| s.clone()).unwrap_or((0, String::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_percent_math() {
        assert_eq!(busy_percent((0, 0), (0, 0)), 0.0);
        assert_eq!(busy_percent((100, 200), (150, 300)), 50.0);
        assert_eq!(busy_percent((0, 0), (0, 100)), 100.0);
        assert_eq!(busy_percent((0, 0), (500, 100)), 0.0); // ocioso nunca passa do total
        assert_eq!(busy_percent((10, 10), (5, 5)), 0.0); // contador regrediu
    }

    #[test]
    fn rate_tolerates_counter_reset() {
        assert_eq!(rate(1000, 3000, 2.0), 1000.0);
        assert_eq!(rate(3000, 1000, 1.0), 0.0);
        assert_eq!(rate(0, 100, 0.0), 0.0);
    }

    /// A página `system.html` lê estes campos; renomear qualquer um quebra o dashboard em silêncio.
    #[test]
    fn json_matches_what_the_page_reads() {
        let v: serde_json::Value = serde_json::to_value(SystemInfo::default()).unwrap();
        for key in ["cpu", "cores", "mem_used", "mem_total", "disk_used", "disk_total", "net_down", "net_up", "uptime_secs"] {
            assert!(v.get(key).is_some(), "campo ausente no JSON: {key}");
        }
        let page = include_str!("../assets/wallpapers/web/system.html");
        for key in ["cpu", "cores", "mem_used", "mem_total", "disk_used", "disk_total", "net_down", "net_up", "uptime_secs"] {
            assert!(page.contains(&format!(".{key}")), "a página não lê o campo {key}");
        }
    }

    /// Leitura real do Windows: valores plausíveis, sem unidades trocadas.
    #[test]
    fn real_sample_is_plausible() {
        let mut s = SystemSampler::default();
        s.sample();
        std::thread::sleep(Duration::from_millis(300));
        let i = s.sample();
        println!("amostra real: {i:?}");
        assert!(!i.cores.is_empty(), "sem contadores por núcleo");
        assert!((0.0..=100.0).contains(&i.cpu), "cpu fora de 0..100: {}", i.cpu);
        assert!(i.cores.iter().all(|c| (0.0..=100.0).contains(c)));
        assert!(i.mem_total > 1 << 28 && i.mem_used <= i.mem_total, "memória implausível: {i:?}");
        assert!(i.disk_total > 0 && i.disk_used <= i.disk_total, "disco implausível: {i:?}");
        assert!(i.uptime_secs > 0);
        assert!(i.net_down >= 0.0 && i.net_up >= 0.0);
    }
}
