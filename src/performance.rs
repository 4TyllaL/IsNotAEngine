//! Modo desempenho: deixa o motor no consumo mínimo enquanto o usuário joga (ou quando quiser).
//!
//! Ativo, o motor não renderiza nada (os wallpapers ficam no último quadro), para os leitores de
//! áudio, mídia e sistema, acorda só uma vez por segundo, roda com prioridade ociosa e em modo
//! de eficiência de energia (EcoQoS) e devolve ao Windows a memória que não está usando.

use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows::Win32::System::Threading::{
    GetCurrentProcess, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling, SetPriorityClass,
    SetProcessInformation,
};
use windows::Win32::UI::Shell::{QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState};

use crate::config::PerformanceMode;

/// Se o modo desempenho deve estar ativo, dada a escolha do usuário e se há app em tela cheia.
pub fn should_be_active(mode: PerformanceMode, fullscreen_app: bool) -> bool {
    match mode {
        PerformanceMode::Off => false,
        PerformanceMode::Auto => fullscreen_app,
        PerformanceMode::Always => true,
    }
}

/// Há um app em tela cheia (jogo, vídeo, apresentação) no primeiro plano? É o mesmo sinal que o
/// Windows usa para segurar notificações, então não depende de lista de jogos.
pub fn fullscreen_app_running() -> bool {
    // SAFETY: consulta sem argumentos; erro significa "não sei", tratado como não.
    match unsafe { SHQueryUserNotificationState() } {
        Ok(state) => state == QUNS_BUSY || state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_PRESENTATION_MODE,
        Err(_) => false,
    }
}

/// Liga (`true`) ou desliga a economia do processo: prioridade, EcoQoS e memória.
pub fn set_low_power(on: bool) {
    // SAFETY: o pseudo-handle do processo atual é sempre válido; as estruturas vivem durante a chamada.
    unsafe {
        let process = GetCurrentProcess();
        let _ = SetPriorityClass(process, if on { IDLE_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS });
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: if on { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
        };
        let _ = SetProcessInformation(
            process,
            ProcessPowerThrottling,
            &state as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
        if on {
            let _ = EmptyWorkingSet(process);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_decides_when_it_is_active() {
        assert!(!should_be_active(PerformanceMode::Off, true));
        assert!(should_be_active(PerformanceMode::Auto, true));
        assert!(!should_be_active(PerformanceMode::Auto, false));
        assert!(should_be_active(PerformanceMode::Always, false));
    }

    #[test]
    fn low_power_lowers_the_process_priority_and_restores_it() {
        use windows::Win32::System::Threading::GetPriorityClass;
        // SAFETY: só leitura da prioridade do próprio processo de teste.
        let class = || unsafe { GetPriorityClass(GetCurrentProcess()) };
        set_low_power(true);
        assert_eq!(class(), IDLE_PRIORITY_CLASS.0);
        set_low_power(false);
        assert_eq!(class(), NORMAL_PRIORITY_CLASS.0);
    }
}
