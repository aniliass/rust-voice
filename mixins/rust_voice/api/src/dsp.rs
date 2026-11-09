//! Свои звуковые блоки без внешних крейтов: фильтр гула и автогромкость для микрофона,
//! и пространственный звук для чужих голосов (затухание, стерео, «лёгкий» HRTF, глухость за стеной).
//!
//! Тут нет ни API игры, ни cpal: всё проверяется обычными тестами.

use std::collections::VecDeque;
use std::f32::consts::{FRAC_PI_4, PI};

pub const RATE: f32 = 48_000.0;

// ---------- микрофон ----------

/// Фильтр верхних частот второго порядка (RBJ biquad): срезает гул ниже `fc` Гц.
pub struct HighPass {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl HighPass {
    pub fn new(fc: f32) -> HighPass {
        let w = 2.0 * PI * fc / RATE;
        let (sin, cos) = w.sin_cos();
        // Q = 0.7071 (Баттерворт): alpha = sin / (2Q)
        let alpha = sin / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        HighPass {
            b0: (1.0 + cos) / 2.0 / a0,
            b1: -(1.0 + cos) / a0,
            b2: (1.0 + cos) / 2.0 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for v in buf.iter_mut() {
            let x = *v;
            let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
            self.x2 = self.x1;
            self.x1 = x;
            self.y2 = self.y1;
            self.y1 = if y.abs() < 1e-20 { 0.0 } else { y };
            *v = y;
        }
    }
}

/// Автогромкость (AGC). Подстраивается только пока вы говорите, поэтому шум между фразами не раздувает.
/// В конце мягкий ограничитель, чтобы громкий смех не хрипел.
pub struct Agc {
    gain: f32,
}

/// Желаемая громкость речи (RMS ≈ -20 дБFS) и пределы усиления (-6..+18 дБ).
const AGC_TARGET: f32 = 0.1;
const AGC_MIN: f32 = 0.5;
const AGC_MAX: f32 = 8.0;

impl Agc {
    pub fn new() -> Agc {
        Agc { gain: 1.0 }
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }

    pub fn reset(&mut self) {
        self.gain = 1.0;
    }

    /// Обработать кадр 20 мс. `voice` — в этом кадре идёт речь (по порогу или по клавише).
    pub fn process(&mut self, frame: &mut [f32], voice: bool) {
        let old = self.gain;
        if voice {
            let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32).sqrt();
            if rms > 1e-4 {
                let want = (AGC_TARGET / rms).clamp(AGC_MIN, AGC_MAX);
                // стало громко — убавляем быстро (за ~3 кадра), стало тихо — прибавляем медленно (~1.5 с)
                let k = if want < self.gain { 0.3 } else { 0.015 };
                self.gain += (want - self.gain) * k;
            }
        }
        let n = frame.len().max(1) as f32;
        for (i, v) in frame.iter_mut().enumerate() {
            let g = old + (self.gain - old) * (i as f32 + 1.0) / n;
            *v = soft_limit(*v * g);
        }
    }
}

/// Когда передавать голос. Три вещи, которых не было:
/// - предзапись: пока молчите, держим последние 60 мс и при срабатывании отправляем их первыми, поэтому
///   первый слог («п», «т», «с») не съедается;
/// - гистерезис: включаемся на пороге, а выключаемся только когда стало на 6 дБ тише, поэтому тихая речь
///   у порога не рвётся на куски;
/// - мягкие края: первые 5 мс нарастают, последние 10 мс гаснут, без щелчков.
pub struct Gate {
    open: bool,
    hang: u32,
    preroll: VecDeque<Vec<f32>>,
}

/// Что делать с кадром.
#[derive(Debug, PartialEq)]
pub enum GateOut {
    /// Молчим, ничего не отправляем.
    Silent,
    /// Только что замолчали: отправить серверу «конец реплики».
    Ended,
    /// Отправить эти кадры по порядку (при включении — сначала предзапись, последним текущий кадр).
    Send(Vec<Vec<f32>>),
}

/// Сколько кадров по 20 мс держим «про запас» и сколько ещё передаём после того, как стало тихо.
pub const PREROLL_FRAMES: usize = 3;
pub const HANGOVER_FRAMES: u32 = 12;
/// Выключаемся, когда громкость упала ниже порога на столько (в разах по амплитуде: 0.5 = -6 дБ).
const CLOSE_RATIO: f32 = 0.5;
const FADE_IN: usize = 240;
const FADE_OUT: usize = 480;

impl Gate {
    pub fn new() -> Gate {
        Gate { open: false, hang: 0, preroll: VecDeque::new() }
    }

    pub fn reset(&mut self) {
        self.open = false;
        self.hang = 0;
        self.preroll.clear();
    }

    /// Очередной кадр. `level` — громкость кадра (RMS), `ptt` — Some(зажата ли клавиша) в режиме рации,
    /// `muted` — микрофон выключен.
    pub fn process(&mut self, frame: Vec<f32>, level: f32, threshold: f32, ptt: Option<bool>, muted: bool) -> GateOut {
        let (start, keep) = match ptt {
            Some(held) => (held, held),
            None => (level >= threshold, level >= threshold * CLOSE_RATIO),
        };
        if muted {
            self.preroll.clear();
            if self.open {
                self.open = false;
                self.hang = 0;
                return GateOut::Ended;
            }
            return GateOut::Silent;
        }
        if !self.open {
            if !start {
                self.preroll.push_back(frame);
                while self.preroll.len() > PREROLL_FRAMES {
                    self.preroll.pop_front();
                }
                return GateOut::Silent;
            }
            self.open = true;
            self.hang = HANGOVER_FRAMES;
            let mut out: Vec<Vec<f32>> = self.preroll.drain(..).collect();
            out.push(frame);
            fade_in(&mut out[0]);
            return GateOut::Send(out);
        }
        let mut frame = frame;
        if keep {
            self.hang = HANGOVER_FRAMES;
        } else {
            self.hang = self.hang.saturating_sub(1);
            if self.hang == 0 {
                self.open = false;
                // этот тихий кадр не отправляем: он станет началом предзаписи
                self.preroll.push_back(frame);
                return GateOut::Ended;
            }
            if self.hang == 1 {
                fade_out(&mut frame); // последний отправляемый кадр плавно гаснет
            }
        }
        GateOut::Send(vec![frame])
    }
}

fn fade_in(f: &mut [f32]) {
    let n = FADE_IN.min(f.len());
    for (i, v) in f.iter_mut().take(n).enumerate() {
        *v *= i as f32 / n as f32;
    }
}

fn fade_out(f: &mut [f32]) {
    let n = FADE_OUT.min(f.len());
    let start = f.len() - n;
    for (i, v) in f[start..].iter_mut().enumerate() {
        *v *= 1.0 - (i + 1) as f32 / n as f32;
    }
}

/// До 0.8 без изменений, дальше плавно прижимается к 1.0.
pub fn soft_limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= 0.8 {
        x
    } else {
        x.signum() * (0.8 + 0.2 * ((a - 0.8) / 0.2).tanh())
    }
}

// ---------- пространственный звук ----------

/// Куда и как слышен источник: громкость, направление и насколько он за стеной.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Громкость от расстояния, 0..=1.
    pub gain: f32,
    /// Синус угла на источник: -1 слева, +1 справа.
    pub side: f32,
    /// Косинус угла: +1 прямо перед вами, -1 за спиной.
    pub front: f32,
    /// Насколько источник закрыт блоками: 0 — открыт, 1 — за стеной.
    pub occ: f32,
    /// Насколько далеко: 0 — рядом, 1 — у границы дистанции. Издалека голос не только тише, но и глуше
    /// (воздух съедает верха), поэтому дальность слышно, а не только видно по громкости.
    pub far: f32,
}

impl Params {
    /// Источник без места в мире (группы, проверка звука, неизвестный игрок): по центру, как есть.
    pub const CENTER: Params = Params { gain: 1.0, side: 0.0, front: 1.0, occ: 0.0, far: 0.0 };
}

/// Какие эффекты включены в меню.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mode {
    pub directional: bool,
    pub hrtf: bool,
    pub occlusion: bool,
}

/// Ближе этого (в блоках) голос не тише, дальше — плавно гаснет к границе дистанции.
const NEAR: f32 = 2.0;

/// Громкость по расстоянию: 1 вблизи, 0 на `max` блоках. Кривая выпуклая: вдали гаснет быстрее, чем вблизи.
pub fn distance_gain(dist: f32, max: f32) -> f32 {
    if max <= NEAR {
        return 1.0;
    }
    let t = ((dist - NEAR) / (max - NEAR)).clamp(0.0, 1.0);
    (1.0 - t).powf(1.6)
}

/// Задержка между ушами для звука строго сбоку: 0.66 мс (голова ~22 см).
const ITD_MAX: f32 = 0.00066 * RATE;
const DELAY_LEN: usize = 64;
/// За сколько сэмплов параметры доезжают до новых (сглаживание, чтобы не щёлкало при повороте головы).
const SMOOTH_SAMPLES: f32 = 0.03 * RATE;
/// Как часто пересчитывать коэффициенты фильтров (в сэмплах).
const BLOCK: usize = 16;

fn one_pole(fc: f32) -> f32 {
    1.0 - (-2.0 * PI * fc / RATE).exp()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Состояние одного источника: сглаженные параметры, линия задержки на оба уха, фильтры.
pub struct Spatializer {
    cur: Params,
    primed: bool,
    delay: [f32; DELAY_LEN],
    w: usize,
    /// Глухость за стеной (общий фильтр), тень головы (левое, правое ухо).
    lp_occ: f32,
    lp_l: f32,
    lp_r: f32,
    /// Коэффициенты фильтров и усиления ушей на текущий блок.
    k_occ: f32,
    k_l: f32,
    k_r: f32,
    g_l: f32,
    g_r: f32,
    d_l: f32,
    d_r: f32,
    n: usize,
}

impl Spatializer {
    pub fn new() -> Spatializer {
        Spatializer {
            cur: Params::CENTER,
            primed: false,
            delay: [0.0; DELAY_LEN],
            w: 0,
            lp_occ: 0.0,
            lp_l: 0.0,
            lp_r: 0.0,
            k_occ: 1.0,
            k_l: 1.0,
            k_r: 1.0,
            g_l: 1.0,
            g_r: 1.0,
            d_l: 0.0,
            d_r: 0.0,
            n: 0,
        }
    }

    /// Пересчитать коэффициенты по сглаженным параметрам.
    fn coeffs(&mut self, mode: Mode) {
        let p = self.cur;
        let occ = if mode.occlusion { p.occ.clamp(0.0, 1.0) } else { 0.0 };
        // за стеной: глуше (срез с 20 кГц до 700 Гц) и тише вдвое
        let occ_fc = lerp(20_000.0, 700.0, occ.sqrt());
        // издалека: верха плавно уходят (у самой границы срез около 3 кГц), голос как будто «вдали»
        let air_fc = lerp(20_000.0, 3_000.0, p.far.clamp(0.0, 1.0).powf(1.3));
        self.k_occ = one_pole(occ_fc.min(air_fc));
        let base = p.gain * (1.0 - 0.5 * occ);
        let (side, front) = if mode.directional { (p.side.clamp(-1.0, 1.0), p.front.clamp(-1.0, 1.0)) } else { (0.0, 1.0) };
        if mode.directional && mode.hrtf {
            // ближнее ухо слышит как есть, дальнее — позже, тише и глуше (тень головы)
            let a = side.abs();
            let itd = ITD_MAX * a;
            let far_gain = 1.0 - 0.45 * a;
            let far_fc = lerp(16_000.0, 1_800.0, a);
            // сзади оба уха чуть глуше: так слышно «спереди или сзади»
            let back = (-front).max(0.0);
            let both_fc = lerp(18_000.0, 5_000.0, back);
            let both_gain = 1.0 - 0.12 * back;
            let (near_k, far_k) = (one_pole(both_fc), one_pole(far_fc.min(both_fc)));
            if side >= 0.0 {
                (self.g_l, self.g_r) = (base * far_gain * both_gain, base * both_gain);
                (self.k_l, self.k_r) = (far_k, near_k);
                (self.d_l, self.d_r) = (itd, 0.0);
            } else {
                (self.g_l, self.g_r) = (base * both_gain, base * far_gain * both_gain);
                (self.k_l, self.k_r) = (near_k, far_k);
                (self.d_l, self.d_r) = (0.0, itd);
            }
        } else {
            // обычная панорама равной мощности; в центре каждое ухо слышит 1.0
            let ang = (side * 0.85 + 1.0) * FRAC_PI_4;
            let s2 = std::f32::consts::SQRT_2;
            (self.g_l, self.g_r) = (base * ang.cos() * s2, base * ang.sin() * s2);
            (self.k_l, self.k_r) = (1.0, 1.0);
            (self.d_l, self.d_r) = (0.0, 0.0);
        }
    }

    fn tap(&self, d: f32) -> f32 {
        let i = d.floor() as usize;
        let f = d - i as f32;
        let at = |k: usize| self.delay[(self.w + DELAY_LEN - k) % DELAY_LEN];
        at(i) * (1.0 - f) + at(i + 1) * f
    }

    /// Один сэмпл источника -> (левый, правый).
    pub fn next(&mut self, v: f32, target: &Params, mode: Mode) -> (f32, f32) {
        if !self.primed {
            self.cur = *target;
            self.primed = true;
            self.coeffs(mode);
        }
        if self.n == 0 {
            let k = (BLOCK as f32 / SMOOTH_SAMPLES).min(1.0);
            self.cur.gain = lerp(self.cur.gain, target.gain, k);
            self.cur.side = lerp(self.cur.side, target.side, k);
            self.cur.front = lerp(self.cur.front, target.front, k);
            self.cur.occ = lerp(self.cur.occ, target.occ, k);
            self.cur.far = lerp(self.cur.far, target.far, k);
            self.coeffs(mode);
        }
        self.n = (self.n + 1) % BLOCK;

        self.lp_occ += (v - self.lp_occ) * self.k_occ;
        let x = self.lp_occ;
        self.w = (self.w + 1) % DELAY_LEN;
        self.delay[self.w] = x;
        let (xl, xr) = (self.tap(self.d_l), self.tap(self.d_r));
        self.lp_l += (xl - self.lp_l) * self.k_l;
        self.lp_r += (xr - self.lp_r) * self.k_r;
        if self.lp_occ.abs() < 1e-20 {
            self.lp_occ = 0.0;
        }
        (self.lp_l * self.g_l, self.lp_r * self.g_r)
    }

    /// Добавить звук из очереди `q` в буфер вывода `out` (`channels` каналов вперемешку), с громкостью `gain`.
    /// Возвращает true, если очередь кончилась раньше буфера.
    pub fn mix(&mut self, q: &mut VecDeque<f32>, out: &mut [f32], channels: usize, gain: f32, target: &Params, mode: Mode) -> bool {
        let ch = channels.max(1);
        for frame in out.chunks_mut(ch) {
            let Some(v) = q.pop_front() else { return true };
            let (l, r) = self.next(v * gain, target, mode);
            if ch == 1 {
                frame[0] += (l + r) * 0.5;
            } else {
                frame[0] += l;
                frame[1] += r;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * PI * hz * i as f32 / RATE).sin() * 0.5).collect()
    }
    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn high_pass_kills_hum_and_keeps_voice() {
        let mut hp = HighPass::new(80.0);
        let mut hum = tone(30.0, 48_000);
        hp.process(&mut hum);
        assert!(rms(&hum[24_000..]) < 0.06, "гул 30 Гц: {}", rms(&hum[24_000..]));
        let mut hp = HighPass::new(80.0);
        let mut voice = tone(400.0, 48_000);
        hp.process(&mut voice);
        assert!(rms(&voice[24_000..]) > 0.33, "речь 400 Гц: {}", rms(&voice[24_000..]));
    }

    #[test]
    fn agc_lifts_quiet_voice_but_not_silence() {
        let mut agc = Agc::new();
        for _ in 0..200 {
            let mut f: Vec<f32> = tone(300.0, 960).iter().map(|s| s * 0.04).collect();
            agc.process(&mut f, true);
        }
        assert!(agc.gain() > 4.0, "тихая речь подтянулась: {}", agc.gain());
        let g = agc.gain();
        for _ in 0..200 {
            let mut f = vec![0.001f32; 960];
            agc.process(&mut f, false);
        }
        assert_eq!(agc.gain(), g, "в тишине усиление не меняется");
        let mut loud: Vec<f32> = tone(300.0, 960).iter().map(|s| s * 2.0).collect();
        agc.process(&mut loud, true);
        assert!(loud.iter().all(|s| s.abs() <= 1.0), "ограничитель не даёт хрипеть");
    }

    fn frames_sent(out: &GateOut) -> usize {
        match out {
            GateOut::Send(v) => v.len(),
            _ => 0,
        }
    }

    #[test]
    fn gate_sends_preroll_so_first_syllable_survives() {
        let mut g = Gate::new();
        for _ in 0..10 {
            assert_eq!(g.process(vec![0.001; 960], 0.001, 0.02, None, false), GateOut::Silent);
        }
        let out = g.process(vec![0.1; 960], 0.1, 0.02, None, false);
        assert_eq!(frames_sent(&out), PREROLL_FRAMES + 1, "60 мс до слова плюс сам кадр");
        if let GateOut::Send(v) = out {
            assert_eq!(v[0][0], 0.0, "начало плавно нарастает, без щелчка");
        }
    }

    #[test]
    fn gate_holds_quiet_speech_between_thresholds() {
        let mut g = Gate::new();
        g.process(vec![0.1; 960], 0.1, 0.02, None, false);
        // голос стих до 0.012: ниже порога, но выше половины порога — передача идёт без конца
        for _ in 0..100 {
            assert_eq!(frames_sent(&g.process(vec![0.012; 960], 0.012, 0.02, None, false)), 1);
        }
        // а тишину отпускаем после «хвоста»
        let mut sent = 0;
        let mut ended = false;
        for _ in 0..30 {
            match g.process(vec![0.001; 960], 0.001, 0.02, None, false) {
                GateOut::Send(_) => sent += 1,
                GateOut::Ended => ended = true,
                GateOut::Silent => {}
            }
        }
        assert!(ended && sent == HANGOVER_FRAMES as usize - 1, "хвост {sent} кадров, потом конец");
    }

    #[test]
    fn gate_mute_and_push_to_talk() {
        let mut g = Gate::new();
        g.process(vec![0.1; 960], 0.1, 0.02, None, false);
        assert_eq!(g.process(vec![0.1; 960], 0.1, 0.02, None, true), GateOut::Ended);
        assert_eq!(g.process(vec![0.1; 960], 0.1, 0.02, None, true), GateOut::Silent);
        let mut g = Gate::new();
        assert_eq!(g.process(vec![0.0; 960], 0.0, 0.02, Some(false), false), GateOut::Silent);
        assert!(frames_sent(&g.process(vec![0.0; 960], 0.0, 0.02, Some(true), false)) >= 1, "рация: тишина тоже уходит");
    }

    #[test]
    fn distance_fades_to_zero_at_the_edge() {
        assert_eq!(distance_gain(1.0, 32.0), 1.0);
        assert!(distance_gain(16.0, 32.0) < 0.5 && distance_gain(16.0, 32.0) > 0.2);
        assert_eq!(distance_gain(40.0, 32.0), 0.0);
        assert_eq!(distance_gain(10.0, 0.0), 1.0, "дистанция неизвестна — не глушим");
    }

    /// Прогнать тон через источник и вернуть громкость левого и правого уха.
    fn ears(p: Params, mode: Mode, hz: f32) -> (f32, f32) {
        let mut s = Spatializer::new();
        let (mut l, mut r) = (Vec::new(), Vec::new());
        for v in tone(hz, 9_600) {
            let (a, b) = s.next(v, &p, mode);
            l.push(a);
            r.push(b);
        }
        (rms(&l[4_800..]), rms(&r[4_800..]))
    }

    const ALL: Mode = Mode { directional: true, hrtf: true, occlusion: true };

    #[test]
    fn center_is_untouched() {
        let (l, r) = ears(Params::CENTER, ALL, 500.0);
        assert!((l - r).abs() < 1e-3 && (l - 0.3535).abs() < 0.02, "{l} {r}");
        let (l, r) = ears(Params::CENTER, Mode::default(), 500.0);
        assert!((l - 0.3535).abs() < 0.01 && (r - 0.3535).abs() < 0.01);
    }

    #[test]
    fn source_on_the_right_is_louder_in_the_right_ear() {
        let right = Params { side: 1.0, front: 0.0, ..Params::CENTER };
        for mode in [ALL, Mode { directional: true, ..Mode::default() }] {
            let (l, r) = ears(right, mode, 3_000.0);
            assert!(r > l * 1.5, "{l} {r}");
        }
        let (l, r) = ears(right, Mode::default(), 3_000.0);
        assert!((l - r).abs() < 1e-3, "направленный звук выключен — моно");
    }

    #[test]
    fn wall_makes_voice_muffled() {
        let behind_wall = Params { occ: 1.0, ..Params::CENTER };
        let (low, _) = ears(behind_wall, ALL, 300.0);
        let (high, _) = ears(behind_wall, ALL, 5_000.0);
        assert!(high < low * 0.3, "высокие частоты за стеной гаснут: {high} vs {low}");
        let (open, _) = ears(behind_wall, Mode { occlusion: false, ..ALL }, 5_000.0);
        assert!(open > 0.3, "заглушение выключено: {open}");
    }

    #[test]
    fn far_voice_loses_highs_but_keeps_lows() {
        let far = Params { far: 1.0, ..Params::CENTER };
        let (near_hi, _) = ears(Params::CENTER, ALL, 8_000.0);
        let (far_hi, _) = ears(far, ALL, 8_000.0);
        let (far_lo, _) = ears(far, ALL, 300.0);
        assert!(far_hi < near_hi * 0.6, "издалека верха тише: {far_hi} vs {near_hi}");
        assert!(far_lo > 0.3, "низы остаются (без фильтра 0.35): {far_lo}");
    }

    #[test]
    fn behind_is_duller_than_in_front() {
        let front = ears(Params { front: 1.0, ..Params::CENTER }, ALL, 8_000.0).0;
        let back = ears(Params { front: -1.0, ..Params::CENTER }, ALL, 8_000.0).0;
        assert!(back < front * 0.8, "{back} vs {front}");
    }

    #[test]
    fn mix_into_mono_and_stereo() {
        let mut s = Spatializer::new();
        let mut q: VecDeque<f32> = vec![0.5; 10].into();
        let mut out = vec![0.0; 8];
        assert!(!s.mix(&mut q, &mut out, 2, 1.0, &Params::CENTER, Mode::default()));
        assert_eq!(q.len(), 6);
        let mut out = vec![0.0; 10];
        assert!(s.mix(&mut q, &mut out, 1, 1.0, &Params::CENTER, Mode::default()), "очередь кончилась");
    }
}
