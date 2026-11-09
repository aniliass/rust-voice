//! Шумоподавление микрофона: винеровский фильтр с оценкой «сигнал/шум» по Ephraim–Malah
//! (decision-directed), как в телефонах.
//!
//! Готовых крейтов (rnnoise, rustfft) в Cargo.lock Horizon нет, поэтому всё здесь своё: короткое
//! преобразование Фурье на 512 отсчётов с шагом 256 (окно sqrt-Hann, перекрытие 50%).
//!
//! Чем это лучше простого вычитания шума. Вычитание режет каждую частоту по её мгновенной громкости:
//! случайные всплески шума то пропускаются, то нет, и остаток «булькает» металлическими нотками
//! (музыкальный шум). Здесь «сигнал/шум» каждой частоты оценивается с памятью о прошлом кадре
//! (`ALPHA` = 0.98): одиночный всплеск почти не открывает частоту, а настоящая речь открывает сразу.
//! Остаток шума получается ровным и тихим, а не звенящим.
//!
//! Шум по каждой частоте — минимум быстро сглаженной мощности за ~1.4 с («минимальная статистика»
//! Мартина): в паузах между слогами мощность падает до шума, поэтому речь в оценку не попадает.
//! Если игрок нажал «Запомнить шум», оценка держится между профилем и профилем +6 дБ: шумодав с первой
//! секунды знает ваш кулер, и даже если говорить без пауз дольше окна, речь не начинает глушить сама себя.
//!
//! Задержка около 10 мс (512 отсчётов при 48 кГц). Обрабатывает кадр за кадром, длина кадра любая.

use std::collections::VecDeque;
use std::f32::consts::PI;

const N: usize = 512;
const HOP: usize = N / 2;
pub const BINS: usize = N / 2 + 1;
/// Оценка шума — минимум сглаженной мощности за последние `SUBWINDOWS` × `SUB_HOPS` шагов (≈1 с,
/// «минимальная статистика» Мартина). Случайный провал громкости забывается, как только уходит из окна,
/// поэтому ни одна частота не застревает «открытой» и не звенит.
const SUBWINDOWS: usize = 8;
const SUB_HOPS: usize = 32;
/// Сглаживание мощности по времени (доля нового значения).
const POWER_SMOOTH: f32 = 0.15;
/// Для поиска минимума мощность сглаживаем быстро: в паузе между слогами (≈100 мс) она должна успеть
/// упасть до шума, иначе громкая речь «поднимет» оценку шума и заглушит сама себя.
const FAST_SMOOTH: f32 = 0.4;
/// Минимум за окно всегда ниже среднего шума: во сколько раз поправляем (подобрано на белом шуме).
const MIN_BIAS: f32 = 4.0;
/// Память оценки «сигнал/шум»: ближе к 1 — меньше музыкального шума, но чуть «смазаннее» начало слов.
const ALPHA: f32 = 0.98;
/// Нижняя граница априорного «сигнал/шум» (-25 дБ): не даёт частотам совсем «захлопываться».
const XI_MIN: f32 = 0.003;
/// Глубина подавления: от -FLOOR_MIN_DB на слабой силе до -(MIN+RANGE) на полной. Глубже нельзя: на
/// слишком тихом фоне любой остаток шума торчит отдельными «нотками».
const FLOOR_MIN_DB: f32 = 6.0;
const FLOOR_RANGE_DB: f32 = 16.0;
/// Сколько секунд слушаем тишину для «Запомнить шум» и какой процентиль берём по каждой частоте
/// (40%: случайный звук посреди записи не испортит профиль).
pub const LEARN_SECS: f32 = 3.0;
/// С профилем оценка шума может подняться над ним не больше чем вчетверо по мощности (+6 дБ).
const PROFILE_HEADROOM: f32 = 4.0;
const LEARN_PERCENTILE: f32 = 0.4;

struct Fft {
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<usize>,
}

impl Fft {
    fn new() -> Fft {
        let bits = N.trailing_zeros();
        let rev = (0..N).map(|i| i.reverse_bits() >> (usize::BITS - bits)).collect();
        let cos = (0..N / 2).map(|k| (2.0 * PI * k as f32 / N as f32).cos()).collect();
        let sin = (0..N / 2).map(|k| (2.0 * PI * k as f32 / N as f32).sin()).collect();
        Fft { cos, sin, rev }
    }

    /// Преобразование на месте. `inverse` — обратное (с делением на N).
    fn run(&self, re: &mut [f32], im: &mut [f32], inverse: bool) {
        for i in 0..N {
            let j = self.rev[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= N {
            let half = len / 2;
            let step = N / len;
            for start in (0..N).step_by(len) {
                for k in 0..half {
                    let wr = self.cos[k * step];
                    let wi = if inverse { self.sin[k * step] } else { -self.sin[k * step] };
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len <<= 1;
        }
        if inverse {
            let s = 1.0 / N as f32;
            for v in re.iter_mut().chain(im.iter_mut()) {
                *v *= s;
            }
        }
    }
}

/// Запись тишины для «Запомнить шум»: входные сэмплы и спектры мощности каждого шага.
struct Learn {
    buf: Vec<f32>,
    spectra: Vec<Vec<f32>>,
    need: usize,
}

pub struct Denoiser {
    fft: Fft,
    win: Vec<f32>,
    inq: Vec<f32>,
    outq: VecDeque<f32>,
    ola: Vec<f32>,
    power: Vec<f32>,
    noise: Vec<f32>,
    fast: Vec<f32>,
    /// Минимум текущего подокна и минимумы прошлых подокон (по каждой частоте), счётчик шагов подокна.
    sub_min: Vec<f32>,
    past_min: Vec<[f32; SUBWINDOWS]>,
    sub_n: usize,
    /// Оценка мощности чистой речи на прошлом шаге (для decision-directed).
    speech: Vec<f32>,
    primed: bool,
    re: Vec<f32>,
    im: Vec<f32>,
    /// Запомненный шум (нижняя граница оценки шума по каждой частоте).
    profile: Option<Vec<f32>>,
    learn: Option<Learn>,
}

impl Denoiser {
    pub fn new() -> Denoiser {
        let mut d = Denoiser {
            fft: Fft::new(),
            // sqrt от периодического окна Ханна: при 50% перекрытии квадраты окон в сумме дают 1
            win: (0..N).map(|i| (PI * i as f32 / N as f32).sin()).collect(),
            inq: Vec::new(),
            outq: VecDeque::new(),
            ola: vec![0.0; N],
            power: vec![0.0; BINS],
            noise: vec![0.0; BINS],
            speech: vec![0.0; BINS],
            fast: vec![0.0; BINS],
            sub_min: vec![0.0; BINS],
            past_min: vec![[0.0; SUBWINDOWS]; BINS],
            sub_n: 0,
            primed: false,
            re: vec![0.0; N],
            im: vec![0.0; N],
            profile: None,
            learn: None,
        };
        d.reset();
        d
    }

    /// Забыть всё накопленное (при выключении, смене микрофона): задержку заполняем тишиной.
    /// Запомненный профиль шума остаётся.
    pub fn reset(&mut self) {
        self.inq.clear();
        self.outq.clear();
        self.outq.extend(std::iter::repeat(0.0).take(N));
        self.ola.iter_mut().for_each(|v| *v = 0.0);
        self.speech.iter_mut().for_each(|v| *v = 0.0);
        self.primed = false;
    }

    /// Задать или убрать запомненный профиль шума (`BINS` значений мощности).
    pub fn set_profile(&mut self, profile: Option<Vec<f32>>) {
        self.profile = profile.filter(|p| p.len() == BINS && p.iter().all(|v| v.is_finite() && *v >= 0.0));
    }

    /// Начать «Запомнить шум»: следующие `LEARN_SECS` секунд звука идут в профиль.
    pub fn start_learning(&mut self) {
        self.learn = Some(Learn { buf: Vec::new(), spectra: Vec::new(), need: (48_000.0 * LEARN_SECS) as usize / HOP });
    }

    /// Сколько записи тишины уже есть: 0..1, None — не записываем.
    pub fn learning(&self) -> Option<f32> {
        self.learn.as_ref().map(|l| l.spectra.len() as f32 / l.need.max(1) as f32)
    }

    /// Дать кадр в запись тишины. Когда записи хватило, профиль готов: он применяется сразу и возвращается
    /// (чтобы сохранить на диск).
    pub fn learn(&mut self, frame: &[f32]) -> Option<Vec<f32>> {
        let mut l = self.learn.take()?;
        l.buf.extend_from_slice(frame);
        while l.buf.len() >= N {
            let spec = self.spectrum(&l.buf[..N]);
            l.spectra.push(spec);
            l.buf.drain(..HOP);
        }
        if l.spectra.len() < l.need {
            self.learn = Some(l);
            return None;
        }
        let mut profile = vec![0.0f32; BINS];
        let mut col = Vec::with_capacity(l.spectra.len());
        for (k, out) in profile.iter_mut().enumerate() {
            col.clear();
            col.extend(l.spectra.iter().map(|s| s[k]));
            col.sort_by(|a, b| a.total_cmp(b));
            // процентиль мощности шума -> её среднее (у шума мощность распределена экспоненциально:
            // 40-й процентиль = -ln(0.6) ≈ 0.51 от среднего)
            *out = col[((col.len() - 1) as f32 * LEARN_PERCENTILE) as usize] / -(1.0 - LEARN_PERCENTILE).ln();
        }
        self.profile = Some(profile.clone());
        self.primed = false; // оценка шума начнётся с профиля
        Some(profile)
    }

    /// Спектр мощности одного окна (для записи тишины).
    fn spectrum(&mut self, samples: &[f32]) -> Vec<f32> {
        for i in 0..N {
            self.re[i] = samples[i] * self.win[i];
            self.im[i] = 0.0;
        }
        self.fft.run(&mut self.re, &mut self.im, false);
        (0..BINS).map(|k| self.re[k] * self.re[k] + self.im[k] * self.im[k]).collect()
    }

    /// Очистить кадр на месте. `strength` от 0 (выключено) до 1 (максимум).
    pub fn process(&mut self, frame: &mut [f32], strength: f32) {
        if strength <= 0.0 {
            if self.primed || !self.inq.is_empty() {
                self.reset();
            }
            return;
        }
        let strength = strength.min(1.0);
        self.inq.extend_from_slice(frame);
        while self.inq.len() >= N {
            self.hop(strength);
            self.inq.drain(..HOP);
        }
        for s in frame.iter_mut() {
            *s = self.outq.pop_front().unwrap_or(0.0);
        }
    }

    fn hop(&mut self, strength: f32) {
        // запас к оценке шума (минимум занижен) и насколько глубоко можно заглушить частоту
        let over = 1.0 + 1.0 * strength;
        let floor = 10f32.powf(-(FLOOR_MIN_DB + FLOOR_RANGE_DB * strength) / 20.0);
        for i in 0..N {
            self.re[i] = self.inq[i] * self.win[i];
            self.im[i] = 0.0;
        }
        self.fft.run(&mut self.re, &mut self.im, false);
        let mut gains = [1.0f32; BINS];
        for k in 0..BINS {
            let p = self.re[k] * self.re[k] + self.im[k] * self.im[k];
            if !self.primed {
                self.power[k] = p;
                self.fast[k] = p;
                let start = self.profile.as_ref().map_or(p, |pr| pr[k]);
                self.sub_min[k] = start;
                self.past_min[k] = [start; SUBWINDOWS];
                self.speech[k] = 0.0;
            } else {
                self.power[k] += (p - self.power[k]) * POWER_SMOOTH;
                self.fast[k] += (p - self.fast[k]) * FAST_SMOOTH;
            }
            self.sub_min[k] = self.sub_min[k].min(self.fast[k]);
            self.noise[k] = self.past_min[k].iter().fold(self.sub_min[k], |a, &b| a.min(b)) * MIN_BIAS;
            let mut n = self.noise[k];
            if let Some(pr) = &self.profile {
                n = n.clamp(pr[k], pr[k] * PROFILE_HEADROOM);
            }
            let n = (n * over).max(1e-12);
            // апостериорное «сигнал/шум» этого шага и априорное с памятью о прошлом (Ephraim–Malah)
            // мгновенная мощность пополам со сглаженной: одиночный всплеск шума меньше «открывает» частоту
            let post = 0.5 * (p + self.power[k]) / n;
            let prior = (ALPHA * self.speech[k] / n + (1.0 - ALPHA) * (post - 1.0).max(0.0)).max(XI_MIN);
            let g = (prior / (1.0 + prior)).max(floor);
            self.speech[k] = g * g * p;
            gains[k] = g;
        }
        self.primed = true;
        self.sub_n += 1;
        if self.sub_n >= SUB_HOPS {
            self.sub_n = 0;
            for k in 0..BINS {
                self.past_min[k].rotate_right(1);
                self.past_min[k][0] = self.sub_min[k];
                self.sub_min[k] = f32::INFINITY;
            }
        }
        // усиление чуть сглаживаем по соседним частотам: одинокая «открытая» частота и есть звенящая нота
        let mut smooth = gains;
        for k in 1..BINS - 1 {
            smooth[k] = 0.25 * gains[k - 1] + 0.5 * gains[k] + 0.25 * gains[k + 1];
        }
        for k in 0..N {
            let g = smooth[if k < BINS { k } else { N - k }];
            self.re[k] *= g;
            self.im[k] *= g;
        }
        self.fft.run(&mut self.re, &mut self.im, true);
        for i in 0..N {
            self.ola[i] += self.re[i] * self.win[i];
        }
        self.outq.extend(self.ola[..HOP].iter().copied());
        self.ola.copy_within(HOP.., 0);
        for v in &mut self.ola[N - HOP..] {
            *v = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Детерминированный «белый шум» без внешних крейтов.
    struct Lcg(u32);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
            (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        }
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn fft_roundtrip() {
        let f = Fft::new();
        let mut re: Vec<f32> = (0..N).map(|i| (i as f32 * 0.37).sin()).collect();
        let orig = re.clone();
        let mut im = vec![0.0; N];
        f.run(&mut re, &mut im, false);
        f.run(&mut re, &mut im, true);
        for i in 0..N {
            assert!((re[i] - orig[i]).abs() < 1e-3, "{i}");
        }
    }

    #[test]
    fn off_leaves_audio_untouched() {
        let mut d = Denoiser::new();
        let mut f = vec![0.25f32; 960];
        d.process(&mut f, 0.0);
        assert!(f.iter().all(|&s| s == 0.25));
    }

    #[test]
    fn steady_noise_is_reduced() {
        let mut d = Denoiser::new();
        let mut rng = Lcg(1);
        let mut last = Vec::new();
        let mut first_rms = 0.0;
        for n in 0..200 {
            let mut f: Vec<f32> = (0..960).map(|_| rng.next() * 0.05).collect();
            if n == 20 {
                first_rms = rms(&f);
            }
            d.process(&mut f, 1.0);
            last = f;
        }
        assert!(rms(&last) < first_rms * 0.25, "шум стал {} из {}", rms(&last), first_rms);
    }

    /// Музыкальный шум: остаток «звенит», когда громкость остатка сильно скачет от кадра к кадру.
    /// У винеровского фильтра с памятью остаток ровный.
    #[test]
    fn residual_noise_is_steady_not_bubbly() {
        let mut d = Denoiser::new();
        let mut rng = Lcg(3);
        let mut levels = Vec::new();
        for n in 0..300 {
            let mut f: Vec<f32> = (0..960).map(|_| rng.next() * 0.05).collect();
            d.process(&mut f, 1.0);
            if n >= 150 {
                levels.push(rms(&f));
            }
        }
        let mean = levels.iter().sum::<f32>() / levels.len() as f32;
        let var = levels.iter().map(|l| (l - mean).powi(2)).sum::<f32>() / levels.len() as f32;
        assert!(var.sqrt() < mean * 0.35, "остаток скачет: {} при среднем {mean}", var.sqrt());
    }

    /// Сплошной гудок дольше ~1.5 с шумодав считает шумом (как свист кулера) — это нормально.
    /// Речь же прерывается между слогами: её «слоги» по 150 мс с паузами 100 мс должны проходить.
    #[test]
    fn speech_like_tone_over_noise_survives() {
        let mut d = Denoiser::new();
        let mut rng = Lcg(7);
        let mut t = 0usize;
        let mut out_tone = 0.0;
        let mut in_tone = 0.0;
        for n in 0..300 {
            // кадр 20 мс: слог 150 мс, пауза 100 мс (цикл 250 мс = 12.5 кадра); проверяем середину слога
            let tone_on = n >= 150 && (n * 20) % 250 < 150;
            let mut f: Vec<f32> = (0..960)
                .map(|i| {
                    let noise = rng.next() * 0.02;
                    let tone = if tone_on { 0.3 * (2.0 * PI * 440.0 * (t + i) as f32 / 48_000.0).sin() } else { 0.0 };
                    noise + tone
                })
                .collect();
            t += 960;
            // выход задержан на ~10 мс, поэтому меряем кадр целиком внутри слога
            let probe = n >= 280 && (40..=100).contains(&((n * 20) % 250));
            if probe && in_tone == 0.0 {
                in_tone = rms(&f);
            }
            d.process(&mut f, 0.6);
            if probe && out_tone == 0.0 {
                out_tone = rms(&f);
            }
        }
        assert!(out_tone > in_tone * 0.7, "сигнал просел: {out_tone} из {in_tone}");
    }

    #[test]
    fn learned_noise_works_from_the_first_second() {
        let mut rng = Lcg(11);
        let mut d = Denoiser::new();
        d.start_learning();
        let mut profile = None;
        for _ in 0..200 {
            let f: Vec<f32> = (0..960).map(|_| rng.next() * 0.05).collect();
            if let Some(p) = d.learn(&f) {
                profile = Some(p);
                break;
            }
        }
        let profile = profile.expect("за 3 с профиль готов");
        assert_eq!(profile.len(), BINS);
        assert_eq!(d.learning(), None);

        // говорим без пауз дольше окна оценки шума (сплошной тон 4 с): без профиля шумодав начинает считать
        // речь шумом, с профилем — нет
        let talk = |with: bool| {
            let mut d = Denoiser::new();
            if with {
                d.set_profile(Some(profile.clone()));
            }
            let mut rng = Lcg(99);
            let mut t = 0usize;
            let (mut inp, mut out) = (0.0, 0.0);
            for n in 0..300 {
                let mut f: Vec<f32> = (0..960)
                    .map(|i| {
                        let tone = if n >= 100 { 0.3 * (2.0 * PI * 440.0 * (t + i) as f32 / 48_000.0).sin() } else { 0.0 };
                        rng.next() * 0.05 + tone
                    })
                    .collect();
                t += 960;
                if n == 299 {
                    inp = rms(&f);
                }
                d.process(&mut f, 1.0);
                if n == 299 {
                    out = rms(&f);
                }
            }
            out / inp
        };
        assert!(talk(true) > 0.8, "с профилем речь цела: {}", talk(true));
        assert!(talk(false) < talk(true), "без профиля долгая речь проседает: {} vs {}", talk(false), talk(true));
    }
}
