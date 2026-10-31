// Copied from ~/garden/nibnib (src/encode.rs on the optimized branch, at
// cace4e1: k-means palette fitting and beam search), with the starting
// palette seeded, and its logging and the unused random palette removed.

use rand::{Rng, SeedableRng, rngs::StdRng};
use rand_distr::{Distribution, Normal};

#[derive(Clone, Copy, Default)]
struct Pix([f32; 3]);

pub fn gaussian_pal(seed: u64) -> [u32; 256] {
    let mut rng = StdRng::seed_from_u64(seed);
    let normal = Normal::new(1f32, 16.0).unwrap();
    let mut result = [0u32; 256];
    let mut i = 0;
    for step in 0..=2 {
        let delta = (2.0f32).powi(step);
        for geom in 0..45 {
            if geom == 22 && step > 0 {
                continue;
            }
            let dx = (geom % 3 - 1) as f32;
            let dy = ((geom / 3) % 5 - 2) as f32;
            let dz = (geom / 15 - 1) as f32;
            let r0 = 0.5 * delta * dx;
            let g0 = 0.5 * delta * dy;
            let b0 = 0.5 * delta * dz;
            let r1 = delta * dx;
            let g1 = delta * dy;
            let b1 = delta * dz;
            let lo = (r0.round_ties_even() as i32)
                .wrapping_add((g0.round_ties_even() as i32) << 5)
                .wrapping_add((b0.round_ties_even() as i32) << 11);
            let hi = (r1.round_ties_even() as i32)
                .wrapping_add((g1.round_ties_even() as i32) << 5)
                .wrapping_add((b1.round_ties_even() as i32) << 11);
            let word = (lo.wrapping_add(hi << 16)) as u32;
            // TODO: check for dups etc
            result[i] = word;
            i += 1;
        }
    }
    while i < 256 {
        let scale_raw: f32 = rng.random();
        let scale = (-2.0 * scale_raw).exp();
        //let scale = scale_raw;
        let yp = normal.sample(&mut rng);
        const CSCALE: f32 = 0.5;
        let rp = yp + CSCALE * normal.sample(&mut rng);
        let gp = 2. * (yp + CSCALE * normal.sample(&mut rng));
        let bp = yp + CSCALE * normal.sample(&mut rng);
        let rm = normal.sample(&mut rng);
        //let gm = 2. * normal.sample(&mut rng);
        //let bm = normal.sample(&mut rng);
        let gm = 2. * rm;
        let bm = rm;
        const MSCALE: f32 = 0.5;
        let r0 = scale * (rp - MSCALE * rm);
        let r1 = r0 + scale * (rp + MSCALE * rm);
        let g0 = scale * (gp - MSCALE * gm);
        let g1 = g0 + scale * (gp + MSCALE * gm);
        let b0 = scale * (bp - MSCALE * bm);
        let b1 = b0 + scale * (bp + MSCALE * bm);
        let lo = (r0.round_ties_even() as i32)
            .wrapping_add((g0.round_ties_even() as i32) << 5)
            .wrapping_add((b0.round_ties_even() as i32) << 11);
        let hi = (r1.round_ties_even() as i32)
            .wrapping_add((g1.round_ties_even() as i32) << 5)
            .wrapping_add((b1.round_ties_even() as i32) << 11);
        let word = (lo.wrapping_add(hi << 16)) as u32;
        // TODO: check for dups etc
        result[i] = word;
        i += 1;
    }
    result
}

/// Bytes per encoded line: the first pixel pair, one index byte per
/// following pair, then padding to a multiple of 4.
pub fn line_stride(width: u32) -> usize {
    (width as usize / 2 + 3).next_multiple_of(4)
}

/// One pair step as the encoder saw it: the code chosen, its cost, and the
/// delta from the base that would have hit the target (with diffused error)
/// exactly, as r, g, b in 565 units for each pixel of the pair.
pub struct Sample {
    code: u8,
    err: f32,
    ideal: [f32; 6],
}

/// Fit the palette to the image, k-means style: encode, set each entry to
/// the mean ideal delta of the steps that chose it, and repeat. Error
/// diffusion feeds back into the targets, so this doesn't converge
/// monotonically; keep the best palette seen. The fitting encodes greedily,
/// and only the final encoding uses the beam search: fitting with the beam
/// is about 6x slower for a hair better.
pub fn encode_kmeans(
    width: u32,
    height: u32,
    image: &[u8],
    mut pal: [u32; 256],
    iters: usize,
    beam: usize,
) -> Vec<u8> {
    let mut samples = Vec::new();
    let mut best = (f64::INFINITY, pal);
    for i in 0..=iters {
        samples.clear();
        encode_with_pal(width, height, image, &pal, 1, Some(&mut samples));
        let err = mean_err(&samples);
        if err < best.0 {
            best = (err, pal);
        }
        if i < iters {
            pal = refit(&samples);
        }
    }
    samples.clear();
    let encoded = encode_with_pal(width, height, image, &best.1, beam, Some(&mut samples));
    encoded
}

fn mean_err(samples: &[Sample]) -> f64 {
    samples.iter().map(|s| s.err as f64).sum::<f64>() / samples.len() as f64
}

/// Set each palette entry to the mean ideal delta of its samples. An unused
/// entry takes the ideal delta of one of the worst-coded samples instead.
fn refit(samples: &[Sample]) -> [u32; 256] {
    let mut sums = [[0f64; 6]; 256];
    let mut counts = [0usize; 256];
    for s in samples {
        let sum = &mut sums[s.code as usize];
        for (acc, v) in sum.iter_mut().zip(s.ideal) {
            *acc += v as f64;
        }
        counts[s.code as usize] += 1;
    }
    let mut worst: Vec<&Sample> = samples.iter().collect();
    worst.sort_by(|a, b| b.err.total_cmp(&a.err));
    let mut worst = worst.into_iter();
    core::array::from_fn(|i| {
        let ideal = if counts[i] > 0 {
            sums[i].map(|v| (v / counts[i] as f64) as f32)
        } else {
            worst.next().map(|s| s.ideal).unwrap_or_default()
        };
        pack_delta(ideal)
    })
}

/// Pack a pair of r, g, b deltas into a palette word, rounding to integers.
fn pack_delta(ideal: [f32; 6]) -> u32 {
    let d = ideal.map(|v| v.round_ties_even() as i32);
    let lo = (d[0] << 11).wrapping_add(d[1] << 5).wrapping_add(d[2]);
    let hi = (d[3] << 11).wrapping_add(d[4] << 5).wrapping_add(d[5]);
    lo.wrapping_add(hi << 16) as u32
}

/// Encode with a given palette, choosing each row's codes by beam search,
/// and optionally recording each pair step.
pub fn encode_with_pal(
    width: u32,
    height: u32,
    image: &[u8],
    pal: &[u32; 256],
    beam: usize,
    mut samples: Option<&mut Vec<Sample>>,
) -> Vec<u8> {
    let stride = line_stride(width);
    let mut result = Vec::with_capacity(1028 + stride * height as usize);
    result.extend((width as u16).to_le_bytes());
    result.extend((height as u16).to_le_bytes());
    // Note: this is a memcpy
    for x in pal {
        result.extend(x.to_le_bytes());
    }
    let mut err_line = vec![Pix::default(); width as usize + 1];
    let mut i = 0;
    for _y in 0..height {
        let line_start = result.len();
        let row = &image[i..i + width as usize * 3];
        let pix0 = Pix::from_888(&image[i..]);
        let pix1 = Pix::from_888(&image[i + 3..]);
        i += 6;
        result.extend(pix0.to_565().to_le_bytes());
        let mut p = pix1.to_565();
        result.extend(p.to_le_bytes());
        let mut xm1_err = pix1 - Pix::from_565(p);
        let mut xm1_y1_err = Pix::default();
        let codes = search_row(row, &err_line, p, xm1_err, pal, beam);
        for x in (2..width as usize).step_by(2) {
            let pix0 = Pix::from_888(&image[i..]);
            let pix1 = Pix::from_888(&image[i + 3..]);
            i += 6;
            let base = p as u32 * 0x10001;
            let x0_y1_err = err_line[x];
            let x1_y1_err = err_line[x + 1];
            let x2_y1_err = err_line[x + 2];
            let err0 = (7. / 16.) * xm1_err
                + 1. / 16. * xm1_y1_err
                + 5. / 16. * x0_y1_err
                + 3. / 16. * x1_y1_err;
            let err1 = 1. / 16. * x0_y1_err + 5. / 16. * x1_y1_err + 3. / 16. * x2_y1_err;
            let pix0_w_err = pix0 + err0;
            let pix1_w_err = pix1 + err1;
            let best_code = codes[x / 2 - 1] as usize;
            let pixpair = base.wrapping_add(pal[best_code]);
            let (best_err, delta0, delta1) = pair_err(pixpair, pix0_w_err, pix1_w_err);
            result.push(best_code as u8);
            if let Some(samples) = samples.as_deref_mut() {
                let base = Pix::from_565(p).to_565_units();
                let t0 = pix0_w_err.to_565_units();
                let t1 = pix1_w_err.to_565_units();
                samples.push(Sample {
                    code: best_code as u8,
                    err: best_err,
                    ideal: core::array::from_fn(|j| {
                        if j < 3 {
                            t0[j] - base[j]
                        } else {
                            t1[j - 3] - base[j - 3]
                        }
                    }),
                });
            }
            xm1_err = (7. / 16.) * delta0 + delta1;
            xm1_y1_err = err_line[x + 1];
            err_line[x] = delta0;
            err_line[x + 1] = xm1_err;
            p = (pixpair >> 16) as u16;
        }
        result.resize(line_start + stride, 0);
    }
    result
}

/// The cost of decoding to `pixpair` where the targets (with diffused error)
/// are `t0` and `t1`, and the error of each pixel.
fn pair_err(pixpair: u32, t0: Pix, t1: Pix) -> (f32, Pix, Pix) {
    let delta0 = t0 - Pix::from_565(pixpair as u16);
    let delta1 = t1 - Pix::from_565((pixpair >> 16) as u16);
    let errp = (delta0 + delta1).hypot2();
    let err_base = (7. / 16.) * delta0.hypot2() + delta1.hypot2();
    (errp + 0.5 * err_base, delta0, delta1)
}

/// The targets for the pair at x, with error diffused from the previous row
/// (`err_line`, not yet overwritten) and `h` carried along this row.
fn pair_targets(row: &[u8], err_line: &[Pix], x: usize, h: Pix) -> (Pix, Pix) {
    let err0 = (7. / 16.) * h
        + 1. / 16. * err_line[x - 1]
        + 5. / 16. * err_line[x]
        + 3. / 16. * err_line[x + 1];
    let err1 = 1. / 16. * err_line[x] + 5. / 16. * err_line[x + 1] + 3. / 16. * err_line[x + 2];
    (
        Pix::from_888(&row[x * 3..]) + err0,
        Pix::from_888(&row[x * 3 + 3..]) + err1,
    )
}

/// A partial path through a row: its total cost so far, the chain state
/// (the last right pixel), and the error carried along the row.
struct Hyp {
    cost: f64,
    state: u16,
    h: Pix,
}

/// Beam search a row for the codes with least total cost, starting from the
/// seed's right pixel and carried error. The previous row's errors don't
/// depend on the path, so a hypothesis needs only its state and carried
/// error. With a beam of 1 this is the greedy encoder.
fn search_row(
    row: &[u8],
    err_line: &[Pix],
    state: u16,
    h: Pix,
    pal: &[u32; 256],
    beam: usize,
) -> Vec<u8> {
    let width = err_line.len() - 1;
    let mut hyps = vec![Hyp { cost: 0., state, h }];
    let mut next = Vec::with_capacity(beam);
    let mut back: Vec<Vec<(u32, u8)>> = Vec::with_capacity(width / 2);
    let mut cands: Vec<(f64, u32, u8)> = Vec::with_capacity(256 * beam);
    // Costs accumulate in f64 so that adding a path's cost doesn't round away
    // the difference between codes. Ties go to the earlier hypothesis and
    // the lower code, as in greedy.
    let order = |a: &(f64, u32, u8), b: &(f64, u32, u8)| {
        a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2))
    };
    for x in (2..width).step_by(2) {
        cands.clear();
        for (j, hyp) in hyps.iter().enumerate() {
            let (t0, t1) = pair_targets(row, err_line, x, hyp.h);
            let base = hyp.state as u32 * 0x10001;
            for code in 0..256 {
                let (err, _, _) = pair_err(base.wrapping_add(pal[code]), t0, t1);
                cands.push((hyp.cost + err as f64, j as u32, code as u8));
            }
        }
        let keep = (4 * beam).min(cands.len());
        if keep < cands.len() {
            cands.select_nth_unstable_by(keep, order);
            cands.truncate(keep);
        }
        cands.sort_unstable_by(order);
        // Keep the best, at most one per chain state. Hypotheses with the same
        // state differ only in carried error, so this is close to Viterbi,
        // and it keeps the beam diverse.
        next.clear();
        let mut step_back = Vec::with_capacity(beam);
        for &(cost, j, code) in &cands {
            let parent = &hyps[j as usize];
            let pixpair = (parent.state as u32 * 0x10001).wrapping_add(pal[code as usize]);
            let state = (pixpair >> 16) as u16;
            if next.iter().any(|h: &Hyp| h.state == state) {
                continue;
            }
            let (t0, t1) = pair_targets(row, err_line, x, parent.h);
            let (_, delta0, delta1) = pair_err(pixpair, t0, t1);
            next.push(Hyp {
                cost,
                state,
                h: (7. / 16.) * delta0 + delta1,
            });
            step_back.push((j, code));
            if next.len() == beam {
                break;
            }
        }
        back.push(step_back);
        core::mem::swap(&mut hyps, &mut next);
    }
    // The hypotheses are in cost order, so the best is first.
    let mut codes = vec![0; back.len()];
    let mut j = 0;
    for (k, step) in back.iter().enumerate().rev() {
        let (parent, code) = step[j];
        codes[k] = code;
        j = parent as usize;
    }
    codes
}

impl Pix {
    fn to_565(self) -> u16 {
        let r = (self.0[0] * (31. / 255.)).round_ties_even() as u16;
        let g = (self.0[1] * (63. / 255.)).round_ties_even() as u16;
        let b = (self.0[2] * (31. / 255.)).round_ties_even() as u16;
        (r << 11) | (g << 5) | b
    }

    fn from_565(pix: u16) -> Self {
        let r = (pix >> 11) as f32 * (255. / 31.);
        let g = ((pix >> 5) & 0x3f) as f32 * (255. / 63.);
        let b = (pix & 0x1f) as f32 * (255. / 31.);
        Self([r, g, b])
    }

    /// Channels scaled to 565 units, clamped to the representable range.
    fn to_565_units(self) -> [f32; 3] {
        let [r, g, b] = self.0.map(|c| c.clamp(0., 255.));
        [r * (31. / 255.), g * (63. / 255.), b * (31. / 255.)]
    }

    fn from_888(slice: &[u8]) -> Self {
        Self([slice[0] as f32, slice[1] as f32, slice[2] as f32])
    }

    fn hypot2(self) -> f32 {
        let [r, g, b] = self.0;
        0.3 * r * r + 0.7 * g * g + 0.1 * b * b
    }
}

impl core::ops::Add for Pix {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self([
            self.0[0] + rhs.0[0],
            self.0[1] + rhs.0[1],
            self.0[2] + rhs.0[2],
        ])
    }
}

impl core::ops::Sub for Pix {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self([
            self.0[0] - rhs.0[0],
            self.0[1] - rhs.0[1],
            self.0[2] - rhs.0[2],
        ])
    }
}

impl core::ops::Mul<Pix> for f32 {
    type Output = Pix;

    fn mul(self, rhs: Pix) -> Pix {
        Pix([self * rhs.0[0], self * rhs.0[1], self * rhs.0[2]])
    }
}
