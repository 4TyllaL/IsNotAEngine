//! Playlist de wallpapers: ordem de reprodução e curva do fade da transição.
//!
//! Lógica pura (sem GPU/Win32) para ser testável; o engine só decide *quando* chamar.

use std::time::Duration;

/// Sorteia a ordem (xorshift64): não precisa ser criptográfico, só variar a cada execução.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn seed_from_clock() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    nanos | 1 // xorshift não aceita estado zero
}

pub struct Playlist {
    items: Vec<String>,
    shuffle: bool,
    order: Vec<usize>,
    pos: usize,
    rng: Rng,
}

impl Playlist {
    pub fn new(items: Vec<String>, shuffle: bool) -> Self {
        Self::with_seed(items, shuffle, seed_from_clock())
    }

    fn with_seed(items: Vec<String>, shuffle: bool, seed: u64) -> Self {
        let mut p = Self { order: (0..items.len()).collect(), items, shuffle, pos: 0, rng: Rng(seed | 1) };
        if p.shuffle {
            p.reshuffle(None);
        }
        p
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Próximo item. Com `avoid`, uma lista recém-embaralhada não começa repetindo o último exibido.
    pub fn next(&mut self, avoid: Option<&str>) -> Option<String> {
        if self.items.is_empty() {
            return None;
        }
        if self.pos >= self.order.len() {
            self.pos = 0;
            if self.shuffle {
                self.reshuffle(avoid);
            }
        }
        let item = self.items[self.order[self.pos]].clone();
        self.pos += 1;
        Some(item)
    }

    fn reshuffle(&mut self, avoid: Option<&str>) {
        for i in (1..self.order.len()).rev() {
            let j = (self.rng.next() % (i as u64 + 1)) as usize;
            self.order.swap(i, j);
        }
        if let (Some(avoid), true) = (avoid, self.order.len() > 1)
            && self.items[self.order[0]] == avoid
        {
            let last = self.order.len() - 1;
            self.order.swap(0, last);
        }
    }
}

/// Quanto o quadro está escurecido (0 = normal, 1 = preto) durante a transição.
/// `half` é a duração de cada metade; antes da troca escurece, depois clareia.
pub fn fade_amount(elapsed: Duration, half: Duration, swapped: bool) -> f32 {
    if half.is_zero() {
        return 0.0;
    }
    let t = (elapsed.as_secs_f32() / half.as_secs_f32()).clamp(0.0, 1.0);
    let t = t * t * (3.0 - 2.0 * t); // smoothstep
    if swapped { 1.0 - t } else { t }
}

/// Converte `"HH:MM"` (00:00..=23:59) em minutos desde a meia-noite (`0..1440`).
pub fn parse_hhmm(s: &str) -> Option<u16> {
    let (h_str, m_str) = s.trim().split_once(':')?;
    let h: u16 = h_str.trim().parse().ok()?;
    let m: u16 = m_str.trim().parse().ok()?;
    if h < 24 && m < 60 {
        Some(h * 60 + m)
    } else {
        None
    }
}

/// Determina qual regra da agenda por horário está ativa para `monitor_id` em `now_minutes` (0..1440).
/// Retorna `(minutos_inicio, wallpaper)`. Quando o relógio ainda está antes do primeiro horário do dia,
/// vale o último horário da noite anterior (virada da meia-noite). Regras específicas do monitor têm
/// prioridade sobre regras globais (`monitor: None`) no mesmo horário.
pub fn active_schedule_slot(
    slots: &[crate::config::ScheduleSlot],
    monitor_id: usize,
    now_minutes: u16,
) -> Option<(u16, &str)> {
    let applicable: Vec<(u16, u8, usize, &str)> = slots
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.wallpaper.is_empty() && (s.monitor.is_none() || s.monitor == Some(monitor_id)))
        .filter_map(|(idx, s)| {
            let start = parse_hhmm(&s.start)?;
            let specific = u8::from(s.monitor == Some(monitor_id));
            Some((start, specific, idx, s.wallpaper.as_str()))
        })
        .collect();

    if applicable.is_empty() {
        return None;
    }

    let chosen = applicable
        .iter()
        .filter(|(start, _, _, _)| *start <= now_minutes)
        .max_by_key(|(start, specific, idx, _)| (*start, *specific, *idx))
        .or_else(|| {
            applicable
                .iter()
                .max_by_key(|(start, specific, idx, _)| (*start, *specific, *idx))
        })?;

    Some((chosen.0, chosen.3))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ScheduleSlot;

    fn items(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("w{i}")).collect()
    }

    #[test]
    fn empty_playlist_yields_nothing() {
        let mut p = Playlist::new(Vec::new(), true);
        assert!(p.is_empty());
        assert_eq!(p.next(None), None);
    }

    #[test]
    fn sequential_order_loops() {
        let mut p = Playlist::new(items(3), false);
        let got: Vec<_> = (0..7).filter_map(|_| p.next(None)).collect();
        assert_eq!(got, ["w0", "w1", "w2", "w0", "w1", "w2", "w0"]);
    }

    #[test]
    fn shuffle_plays_every_item_once_per_round() {
        let mut p = Playlist::with_seed(items(6), true, 42);
        let mut round: Vec<_> = (0..6).filter_map(|_| p.next(None)).collect();
        round.sort();
        assert_eq!(round, items(6));
    }

    #[test]
    fn shuffle_does_not_repeat_across_rounds() {
        for seed in 1..200 {
            let mut p = Playlist::with_seed(items(4), true, seed);
            let mut last = None;
            for _ in 0..40 {
                let n = p.next(last.as_deref()).unwrap();
                assert_ne!(Some(&n), last.as_ref(), "repetiu na seed {seed}");
                last = Some(n);
            }
        }
    }

    #[test]
    fn single_item_repeats_without_panicking() {
        let mut p = Playlist::new(items(1), true);
        assert_eq!(p.next(Some("w0")).as_deref(), Some("w0"));
        assert_eq!(p.next(Some("w0")).as_deref(), Some("w0"));
    }

    #[test]
    fn fade_goes_dark_then_clears() {
        let half = Duration::from_millis(400);
        assert_eq!(fade_amount(Duration::ZERO, half, false), 0.0);
        assert_eq!(fade_amount(half, half, false), 1.0);
        assert_eq!(fade_amount(Duration::ZERO, half, true), 1.0);
        assert_eq!(fade_amount(half, half, true), 0.0);
        assert!((fade_amount(half / 2, half, false) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn zero_duration_never_fades() {
        assert_eq!(fade_amount(Duration::from_secs(1), Duration::ZERO, false), 0.0);
    }

    #[test]
    fn schedule_parses_hhmm_and_handles_midnight_wrap_and_per_monitor_override() {
        assert_eq!(parse_hhmm("08:30"), Some(8 * 60 + 30));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("12:60"), None);

        let slots = vec![
            ScheduleSlot { start: "07:00".into(), wallpaper: "coding".into(), monitor: None },
            ScheduleSlot { start: "19:00".into(), wallpaper: "tokyo_rain".into(), monitor: None },
            ScheduleSlot { start: "19:00".into(), wallpaper: "winter_cabin".into(), monitor: Some(1) },
            ScheduleSlot { start: "23:00".into(), wallpaper: "night_sky".into(), monitor: None },
        ];

        // Às 02:00 (antes das 07:00), vale o slot das 23:00 da noite anterior
        assert_eq!(active_schedule_slot(&slots, 0, 2 * 60), Some((23 * 60, "night_sky")));
        // Às 10:00, vale o slot das 07:00
        assert_eq!(active_schedule_slot(&slots, 0, 10 * 60), Some((7 * 60, "coding")));
        // Às 20:00, monitor 0 usa a regra global ("tokyo_rain") e monitor 1 usa sua regra específica ("winter_cabin")
        assert_eq!(active_schedule_slot(&slots, 0, 20 * 60), Some((19 * 60, "tokyo_rain")));
        assert_eq!(active_schedule_slot(&slots, 1, 20 * 60), Some((19 * 60, "winter_cabin")));
    }
}
