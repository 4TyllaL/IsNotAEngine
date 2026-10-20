//! Política de pausa do motor, sem dependência de Win32 (testável isoladamente).
//!
//! Há duas fontes de pausa independentes: a **manual** (Studio/bandeja) e a **automática**
//! (desktop coberto por janelas e/ou notebook na bateria, cada uma com sua opção).
//! Manual e bateria valem para todas as telas; a cobertura vale **por monitor**.

use std::time::{Duration, Instant};

/// Resultado da avaliação da pausa automática.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AutoPause {
    pub covered: bool,
    pub battery: bool,
}

impl AutoPause {
    /// Avalia as condições; cada uma só conta se a respectiva opção estiver ligada.
    pub fn evaluate(pause_when_covered: bool, all_covered: bool, pause_on_battery: bool, on_battery: bool) -> Self {
        Self {
            covered: pause_when_covered && all_covered,
            battery: pause_on_battery && on_battery,
        }
    }

    pub fn active(self) -> bool {
        self.covered || self.battery
    }
}

/// A renderização fica suspensa se a pausa manual ou a automática estiver ativa.
pub fn is_suspended(manual: bool, auto_active: bool) -> bool {
    manual || auto_active
}

/// Pausa de um monitor: manual e bateria param todos; a cobertura só para a tela coberta.
pub fn monitor_suspended(manual: bool, battery: bool, covered: bool) -> bool {
    manual || battery || covered
}

/// Relógio de um monitor que não conta o tempo suspenso: ao retomar, a animação continua de
/// onde parou em vez de saltar para o tempo de parede.
#[derive(Debug, Default)]
pub struct PauseClock {
    paused_total: Duration,
    suspended_since: Option<Instant>,
}

impl PauseClock {
    pub fn set_suspended(&mut self, suspended: bool, now: Instant) {
        match (suspended, self.suspended_since) {
            (true, None) => self.suspended_since = Some(now),
            (false, Some(since)) => {
                self.paused_total += now.saturating_duration_since(since);
                self.suspended_since = None;
            }
            _ => {}
        }
    }

    /// Segundos desde `start`, descontado o tempo suspenso (congelado enquanto suspenso).
    pub fn elapsed(&self, start: Instant, now: Instant) -> f32 {
        let upto = self.suspended_since.unwrap_or(now);
        upto.saturating_duration_since(start).saturating_sub(self.paused_total).as_secs_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covered_only_suspends_its_own_monitor() {
        // Duas telas, só a 0 coberta.
        assert!(monitor_suspended(false, false, true));
        assert!(!monitor_suspended(false, false, false));
        // Manual e bateria valem para todas.
        assert!(monitor_suspended(true, false, false));
        assert!(monitor_suspended(false, true, false));
    }

    #[test]
    fn clock_freezes_while_suspended_and_resumes_without_jumping() {
        let t0 = Instant::now();
        let at = |s: f32| t0 + Duration::from_secs_f32(s);
        let mut c = PauseClock::default();
        assert!((c.elapsed(t0, at(2.0)) - 2.0).abs() < 1e-3);
        c.set_suspended(true, at(2.0));
        assert!((c.elapsed(t0, at(10.0)) - 2.0).abs() < 1e-3, "deve congelar");
        c.set_suspended(false, at(10.0));
        assert!((c.elapsed(t0, at(11.0)) - 3.0).abs() < 1e-3, "retoma de 2s, sem saltar");
        // Chamadas repetidas não acumulam.
        c.set_suspended(false, at(12.0));
        assert!((c.elapsed(t0, at(12.0)) - 4.0).abs() < 1e-3);
    }

    #[test]
    fn options_gate_each_condition() {
        // Cobertura só pausa com a opção ligada.
        assert!(AutoPause::evaluate(true, true, false, false).active());
        assert!(!AutoPause::evaluate(false, true, false, false).active());
        // Bateria idem, e independe de cobertura.
        assert!(AutoPause::evaluate(false, false, true, true).active());
        assert!(!AutoPause::evaluate(false, false, false, true).active());
        assert!(!AutoPause::evaluate(true, false, true, false).active());
    }

    #[test]
    fn reports_which_condition_fired() {
        let a = AutoPause::evaluate(true, true, true, false);
        assert_eq!(a, AutoPause { covered: true, battery: false });
    }

    #[test]
    fn manual_and_auto_are_independent() {
        assert!(!is_suspended(false, false));
        assert!(is_suspended(true, false));
        assert!(is_suspended(false, true));
        assert!(is_suspended(true, true));
    }
}
