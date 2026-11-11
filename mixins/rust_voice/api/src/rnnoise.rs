//! Шумоподавление нейросетью RNNoise (как в Plasmo Voice): встроенный nnnoiseless 0.5.2.
//!
//! Моды Horizon могут брать только крейты из Cargo.lock самой игры, а nnnoiseless там нет, поэтому
//! его код лежит здесь: `rnn_denoise.rs`, `rnn_features.rs`, `rnn_pitch.rs`, `rnn_net.rs`, `rnn_util.rs`
//! и обученная модель в `rnn_weights.rs` (weights.rnn как массив). Отличия от оригинала: своё БПФ на 960
//! точек вместо easyfft (`fft` ниже), `OnceLock` вместо once_cell, без обучения, C API и dasp.
//!
//! [`Rnnoise`] — обёртка под наш звук: кадры по 960 отсчётов (20 мс) в [-1, 1], а RNNoise ест по 480
//! в масштабе i16. Задержка — один кадр RNNoise (10 мс). Сила 0..1 смешивает чистый и исходный звук.
//!
//! Лицензия nnnoiseless (COPYING):
//
// Copyright (c) 2020, Joe Neeman
// Copyright (c) 2017, Mozilla
// Copyright (c) 2007-2017, Jean-Marc Valin
// Copyright (c) 2005-2017, Xiph.Org Foundation
// Copyright (c) 2003-2004, Mark Borgerding
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions
// are met:
//
// - Redistributions of source code must retain the above copyright
// notice, this list of conditions and the following disclaimer.
//
// - Redistributions in binary form must reproduce the above copyright
// notice, this list of conditions and the following disclaimer in the
// documentation and/or other materials provided with the distribution.
//
// - Neither the name of the Xiph.Org Foundation nor the names of its
// contributors may be used to endorse or promote products derived from
// this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
// ``AS IS'' AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
// LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
// A PARTICULAR PURPOSE ARE DISCLAIMED.  IN NO EVENT SHALL THE FOUNDATION
// OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
// LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
// DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
// THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
// (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
// OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

#![allow(dead_code, clippy::all)]

use std::sync::OnceLock;

#[path = "rnn_util.rs"]
mod util;
#[path = "rnn_denoise.rs"]
mod denoise;
#[path = "rnn_features.rs"]
mod features;
#[path = "rnn_pitch.rs"]
mod pitch;
#[path = "rnn_net.rs"]
mod rnn;
#[path = "rnn_weights.rs"]
mod weights;

pub use denoise::DenoiseState;
pub use rnn::RnnModel;

#[doc(hidden)]
pub const FRAME_SIZE_SHIFT: usize = 2;
#[doc(hidden)]
pub const FRAME_SIZE: usize = 120 << FRAME_SIZE_SHIFT;
pub(crate) const WINDOW_SIZE: usize = 2 * FRAME_SIZE;
#[doc(hidden)]
pub const FREQ_SIZE: usize = FRAME_SIZE + 1;

pub(crate) const PITCH_MIN_PERIOD: usize = 60;
pub(crate) const PITCH_MAX_PERIOD: usize = 768;
pub(crate) const PITCH_FRAME_SIZE: usize = 960;
pub(crate) const PITCH_BUF_SIZE: usize = PITCH_MAX_PERIOD + PITCH_FRAME_SIZE;

#[doc(hidden)]
pub const NB_BANDS: usize = 22;
pub(crate) const CEPS_MEM: usize = 8;
const NB_DELTA_CEPS: usize = 6;
#[doc(hidden)]
pub const NB_FEATURES: usize = NB_BANDS + 3 * NB_DELTA_CEPS + 2;
#[doc(hidden)]
pub const EBAND_5MS: [usize; 22] = [
    // 0  200 400 600 800  1k 1.2 1.4 1.6  2k 2.4 2.8 3.2  4k 4.8 5.6 6.8  8k 9.6 12k 15.6 20k*/
    0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 20, 24, 28, 34, 40, 48, 60, 78, 100,
];
pub(crate) use fft::Complex;

/// Computes the correlation between two frequency-domain signals, and aggregates the correlation
/// into bands.
///
/// `out` is the output (duh), and it has length `NB_BANDS`.
pub(crate) fn compute_band_corr(out: &mut [f32], x: &[Complex], p: &[Complex]) {
    for y in out.iter_mut() {
        *y = 0.0;
    }

    for i in 0..(NB_BANDS - 1) {
        let band_size = (EBAND_5MS[i + 1] - EBAND_5MS[i]) << FRAME_SIZE_SHIFT;
        for j in 0..band_size {
            let frac = j as f32 / band_size as f32;
            let idx = (EBAND_5MS[i] << FRAME_SIZE_SHIFT) + j;
            let corr = x[idx].re * p[idx].re + x[idx].im * p[idx].im;
            out[i] += (1.0 - frac) * corr;
            out[i + 1] += frac * corr;
        }
    }
    out[0] *= 2.0;
    out[NB_BANDS - 1] *= 2.0;
}

fn interp_band_gain(out: &mut [f32], band_e: &[f32]) {
    for y in out.iter_mut() {
        *y = 0.0;
    }

    for i in 0..(NB_BANDS - 1) {
        let band_size = (EBAND_5MS[i + 1] - EBAND_5MS[i]) << FRAME_SIZE_SHIFT;
        for j in 0..band_size {
            let frac = j as f32 / band_size as f32;
            let idx = (EBAND_5MS[i] << FRAME_SIZE_SHIFT) + j;
            out[idx] = (1.0 - frac) * band_e[i] + frac * band_e[i + 1];
        }
    }
}

struct CommonState {
    window: [f32; WINDOW_SIZE],
    dct_table: [f32; NB_BANDS * NB_BANDS],
    wnorm: f32,
}

static COMMON: OnceLock<CommonState> = OnceLock::new();

fn common() -> &'static CommonState {
    if COMMON.get().is_none() {
        let pi = std::f64::consts::PI;
        let mut window = [0.0; WINDOW_SIZE];
        for i in 0..FRAME_SIZE {
            let sin = (0.5 * pi * (i as f64 + 0.5) / FRAME_SIZE as f64).sin();
            window[i] = (0.5 * pi * sin * sin).sin() as f32;
            window[WINDOW_SIZE - i - 1] = (0.5 * pi * sin * sin).sin() as f32;
        }
        let wnorm = 1_f32 / window.iter().map(|x| x * x).sum::<f32>();

        let mut dct_table = [0.0; NB_BANDS * NB_BANDS];
        for i in 0..NB_BANDS {
            for j in 0..NB_BANDS {
                dct_table[i * NB_BANDS + j] =
                    ((i as f64 + 0.5) * j as f64 * pi / NB_BANDS as f64).cos() as f32;
                if j == 0 {
                    dct_table[i * NB_BANDS + j] *= 0.5f32.sqrt();
                }
            }
        }

        let _ = COMMON.set(CommonState {
            window,
            dct_table,
            wnorm,
        });
    }
    COMMON.get().unwrap()
}

/// A brute-force DCT (discrete cosine transform) of size NB_BANDS.
pub(crate) fn dct(out: &mut [f32], x: &[f32]) {
    let c = common();
    for i in 0..NB_BANDS {
        let mut sum = 0.0;
        for j in 0..NB_BANDS {
            sum += x[j] * c.dct_table[j * NB_BANDS + i];
        }
        out[i] = (sum as f64 * (2.0 / NB_BANDS as f64).sqrt()) as f32;
    }
}

fn apply_window(output: &mut [f32], input: &[f32]) {
    let c = common();
    for (x, &y, &w) in util::zip3(output, input, &c.window[..]) {
        *x = y * w;
    }
}

fn apply_window_in_place(xs: &mut [f32]) {
    let c = common();
    for (x, &w) in xs.iter_mut().zip(&c.window[..]) {
        *x *= w;
    }
}


/// Своё БПФ: комплексное смешанного основания (2, 3, 4, 5) для 960 точек и вещественное поверх него.
/// Прямое и обратное — без нормировки (как у rustfft, на который рассчитан код nnnoiseless).
pub mod fft {
    use super::{FREQ_SIZE, WINDOW_SIZE};
    use std::sync::OnceLock;

    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct Complex {
        pub re: f32,
        pub im: f32,
    }
    impl Complex {
        pub const fn new(re: f32, im: f32) -> Complex {
            Complex { re, im }
        }
        fn conj(self) -> Complex {
            Complex::new(self.re, -self.im)
        }
    }
    impl std::ops::Add for Complex {
        type Output = Complex;
        fn add(self, o: Complex) -> Complex {
            Complex::new(self.re + o.re, self.im + o.im)
        }
    }
    impl std::ops::AddAssign for Complex {
        fn add_assign(&mut self, o: Complex) {
            self.re += o.re;
            self.im += o.im;
        }
    }
    impl std::ops::Mul for Complex {
        type Output = Complex;
        fn mul(self, o: Complex) -> Complex {
            Complex::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
        }
    }
    impl std::ops::Mul<f32> for Complex {
        type Output = Complex;
        fn mul(self, k: f32) -> Complex {
            Complex::new(self.re * k, self.im * k)
        }
    }

    /// Половина спектра вещественного сигнала из `WINDOW_SIZE` отсчётов: бины 0..=N/2.
    #[derive(Clone)]
    pub struct Spectrum(pub [Complex; FREQ_SIZE]);

    impl Spectrum {
        pub fn new() -> Spectrum {
            Spectrum([Complex::default(); FREQ_SIZE])
        }
        pub fn scale(&mut self, k: f32) {
            for c in self.0.iter_mut() {
                *c = *c * k;
            }
        }
        pub fn scale_bins(&mut self, k: &[f32]) {
            for (c, k) in self.0.iter_mut().zip(k) {
                *c = *c * *k;
            }
        }
        /// Прямое БПФ вещественного `input` (длина `WINDOW_SIZE`).
        pub fn forward_from(&mut self, input: &[f32]) {
            let mut buf = [Complex::default(); WINDOW_SIZE];
            for (b, x) in buf.iter_mut().zip(input) {
                *b = Complex::new(*x, 0.0);
            }
            let out = plan().run(&buf, false);
            self.0.copy_from_slice(&out[..FREQ_SIZE]);
        }
        /// Обратное БПФ (спектр дополняется сопряжённой половиной) в вещественный `out`.
        pub fn inverse_into(&self, out: &mut [f32]) {
            let mut buf = [Complex::default(); WINDOW_SIZE];
            buf[..FREQ_SIZE].copy_from_slice(&self.0);
            for k in FREQ_SIZE..WINDOW_SIZE {
                buf[k] = self.0[WINDOW_SIZE - k].conj();
            }
            let res = plan().run(&buf, true);
            for (o, c) in out.iter_mut().zip(res.iter()) {
                *o = c.re;
            }
        }
    }

    /// План БПФ: основания (по порядку) и повороты e^(-2πik/N).
    pub struct Plan {
        n: usize,
        factors: Vec<usize>,
        tw: Vec<Complex>,
    }

    fn plan() -> &'static Plan {
        static P: OnceLock<Plan> = OnceLock::new();
        P.get_or_init(|| Plan::new(WINDOW_SIZE))
    }

    impl Plan {
        pub fn new(n: usize) -> Plan {
            let mut factors = Vec::new();
            let mut m = n;
            for p in [4usize, 2, 3, 5] {
                while m % p == 0 {
                    factors.push(p);
                    m /= p;
                }
            }
            assert_eq!(m, 1, "БПФ: {n} не раскладывается на 2, 3, 4, 5");
            let tw = (0..n)
                .map(|k| {
                    let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
                    Complex::new(a.cos() as f32, a.sin() as f32)
                })
                .collect();
            Plan { n, factors, tw }
        }

        /// БПФ `input` (длина `n`); `inverse` — с e^(+…), без деления на n.
        pub fn run(&self, input: &[Complex], inverse: bool) -> Vec<Complex> {
            let mut out = vec![Complex::default(); self.n];
            self.work(&mut out, input, 0, 1, &self.factors, inverse);
            out
        }

        fn twiddle(&self, k: usize, inverse: bool) -> Complex {
            let t = self.tw[k % self.n];
            if inverse { t.conj() } else { t }
        }

        /// Прореживание по времени: `out` — m·p точек из `input[off + i·stride]`.
        fn work(&self, out: &mut [Complex], input: &[Complex], off: usize, stride: usize, factors: &[usize], inverse: bool) {
            let len = out.len();
            if factors.is_empty() {
                out[0] = input[off];
                return;
            }
            let p = factors[0];
            let m = len / p;
            // p подпоследовательностей длины m
            for q in 0..p {
                self.work(&mut out[q * m..(q + 1) * m], input, off + q * stride, stride * p, &factors[1..], inverse);
            }
            // бабочки: для каждого k < m собираем p выходов
            let step = self.n / len; // поворот e^(-2πi/len) = tw[step]
            let mut tmp = [Complex::default(); 5];
            for k in 0..m {
                for q in 0..p {
                    tmp[q] = out[q * m + k] * self.twiddle(q * k * step, inverse);
                }
                for r in 0..p {
                    let mut acc = Complex::default();
                    for q in 0..p {
                        acc += tmp[q] * self.twiddle(q * r * m * step, inverse);
                    }
                    out[r * m + k] = acc;
                }
            }
        }
    }
}

/// RNNoise под наш микрофон: кадры по 960 отсчётов в [-1, 1].
pub struct Rnnoise {
    st: Box<DenoiseState<'static>>,
    /// очищенное, ещё не отданное (задержка — один кадр RNNoise)
    out: std::collections::VecDeque<f32>,
    /// исходный звук с той же задержкой: для смешивания по силе
    dry: std::collections::VecDeque<f32>,
    inq: Vec<f32>,
    /// Вероятность речи в последнем обработанном звуке (наибольшая из кадров RNNoise за вызов).
    vad: f32,
    /// Состояние генератора крошечного шума против денормалов.
    dither: u32,
}

impl Rnnoise {
    pub fn new() -> Rnnoise {
        let mut r = Rnnoise { st: DenoiseState::new(), out: Default::default(), dry: Default::default(), inq: Vec::new(), vad: 0.0, dither: 0x1234_5678 };
        r.reset();
        r
    }

    /// Забыть всё (новый микрофон, выключили и включили): начинаем с тишины.
    pub fn reset(&mut self) {
        self.st = DenoiseState::new();
        self.out.clear();
        self.dry.clear();
        self.inq.clear();
        self.vad = 0.0;
        // один кадр RNNoise задержки: отдаём тишину, пока не накопится
        self.out.extend(std::iter::repeat_n(0.0, FRAME_SIZE));
        self.dry.extend(std::iter::repeat_n(0.0, FRAME_SIZE));
    }

    /// Насколько нейросеть уверена, что сейчас речь (0..1).
    pub fn vad(&self) -> f32 {
        self.vad
    }

    /// Очистить кадр на месте; `strength` 0..1 — доля очищенного звука (1 — только он).
    pub fn process(&mut self, frame: &mut [f32], strength: f32) {
        let k = strength.clamp(0.0, 1.0);
        self.inq.extend_from_slice(frame);
        let mut buf_in = [0.0f32; FRAME_SIZE];
        let mut buf_out = [0.0f32; FRAME_SIZE];
        let mut vad = None::<f32>;
        while self.inq.len() >= FRAME_SIZE {
            // крошечный шум (~ -150 дБ, неслышно): в полной тишине числа внутри сети уходят в денормалы,
            // и каждый кадр считается в 15-20 раз дольше — звуковой поток не успевает, слышны щелчки
            for (i, (b, x)) in buf_in.iter_mut().zip(&self.inq[..FRAME_SIZE]).enumerate() {
                self.dither = self.dither.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let d = ((self.dither >> 9) as f32 / 4_194_304.0 - 1.0) * 1e-3;
                *b = *x * 32768.0 + d + if i % 2 == 0 { 1e-3 } else { -1e-3 };
            }
            self.dry.extend(self.inq[..FRAME_SIZE].iter().copied());
            self.inq.drain(..FRAME_SIZE);
            let p = self.st.process_frame(&mut buf_out, &buf_in);
            vad = Some(vad.map_or(p, |v| v.max(p)));
            self.out.extend(buf_out.iter().map(|v| v / 32768.0));
        }
        if let Some(v) = vad {
            self.vad = v;
        }
        for s in frame.iter_mut() {
            let wet = self.out.pop_front().unwrap_or(0.0);
            let dry = self.dry.pop_front().unwrap_or(0.0);
            *s = (wet * k + dry * (1.0 - k)).clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod port_tests {
    use super::fft::{Complex, Plan};
    use super::*;

    #[test]
    fn fft_matches_plain_dft() {
        let n = 960;
        let x: Vec<Complex> = (0..n).map(|i| Complex::new(((i * 7) % 13) as f32 - 6.0, ((i * 3) % 5) as f32)).collect();
        let fast = Plan::new(n).run(&x, false);
        for k in [0usize, 1, 7, 100, 479, 480, 959] {
            let mut acc = (0.0f64, 0.0f64);
            for (i, v) in x.iter().enumerate() {
                let a = -2.0 * std::f64::consts::PI * (k * i) as f64 / n as f64;
                acc.0 += v.re as f64 * a.cos() - v.im as f64 * a.sin();
                acc.1 += v.re as f64 * a.sin() + v.im as f64 * a.cos();
            }
            assert!((fast[k].re as f64 - acc.0).abs() < 0.05 && (fast[k].im as f64 - acc.1).abs() < 0.05, "бин {k}: {:?} против {acc:?}", fast[k]);
        }
        // обратное от прямого — исходник, умноженный на n
        let back = Plan::new(n).run(&fast, true);
        assert!((back[5].re / n as f32 - x[5].re).abs() < 1e-3);
    }

    #[test]
    fn rnnoise_quiets_noise_and_keeps_a_tone() {
        let mut r = Rnnoise::new();
        let mut seed = 12345u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            ((seed >> 16) as f32 / 32768.0 - 1.0) * 0.05
        };
        // белый шум: после разгона должен стать заметно тише
        let mut in_e = 0.0f32;
        let mut out_e = 0.0f32;
        for f in 0..100 {
            let mut frame: Vec<f32> = (0..960).map(|_| noise()).collect();
            let e_in: f32 = frame.iter().map(|v| v * v).sum();
            r.process(&mut frame, 1.0);
            if f > 20 {
                in_e += e_in;
                out_e += frame.iter().map(|v| v * v).sum::<f32>();
            }
        }
        assert!(out_e < in_e * 0.3, "шум не приглушён: {out_e} из {in_e}");
    }

    #[test]
    fn vad_tells_voice_from_noise() {
        // как в сверке с оригиналом: шум — низкая уверенность, «голос» (тон с модуляцией поверх шума) — высокая
        let mut r = Rnnoise::new();
        let mut seed = 1u32;
        let mut run = |voice: bool, r: &mut Rnnoise, from: usize| -> f32 {
            let (mut sum, mut n) = (0.0f32, 0);
            for f in 0..120 {
                let mut frame: Vec<f32> = (0..960)
                    .map(|i| {
                        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                        let n = ((seed >> 16) as f32 / 32768.0 - 1.0) * 0.06;
                        let t = ((from + f) * 960 + i) as f32 / 48_000.0;
                        let v = if voice { (t * 220.0 * 6.283).sin() * 0.25 * (1.0 + (t * 3.0 * 6.283).sin()) } else { 0.0 };
                        n + v
                    })
                    .collect();
                r.process(&mut frame, 1.0);
                if f > 70 {
                    sum += r.vad();
                    n += 1;
                }
            }
            sum / n as f32 // средняя уверенность: отдельные всплески на шуме бывают
        };
        let noise = run(false, &mut r, 0);
        let voice = run(true, &mut r, 120);
        assert!(noise < 0.3, "шум принят за речь: {noise}");
        assert!(voice > 0.6, "голос не узнан: {voice}");
    }
}
