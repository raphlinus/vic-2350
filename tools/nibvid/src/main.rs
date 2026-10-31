// Copyright 2026 Raph Levien
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A synthetic test video for sd_video: 640x480 at 60 fps, each frame a
//! nibnib image padded to whole 512-byte blocks, so frame n starts at
//! block n * FRAME_BLOCKS of the file.
//!
//! Each frame shows a moving plasma (smooth, which nibnib handles well),
//! a bouncing ball, a white bar sweeping across 8 pixels a frame (tearing
//! would break it), the frame number in big digits, and a progress bar
//! along the bottom, so a capture shows dropped, repeated or torn frames.
//!
//! Each frame gets its own palette, fitted by k-means, and its codes are
//! chosen by beam search, with nibnib's default settings.
//!
//! Usage: nibvid OUT [FRAMES] [PREVIEW_DIR]. With PREVIEW_DIR, also
//! decodes a few frames back to PNGs there.

use std::{fs::File, io::BufWriter, io::Write};

mod decode;
mod encode;

const WIDTH: usize = 640;
const HEIGHT: usize = 480;
const FPS: f32 = 60.0;
/// A frame's encoded size, 156548 bytes, rounded up to whole blocks
const FRAME_BLOCKS: usize = 306;
const FRAME_BYTES: usize = FRAME_BLOCKS * 512;
/// Iterations of k-means and the beam width, as nibnib's defaults
const KMEANS: usize = 10;
const BEAM: usize = 16;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: nibvid OUT [FRAMES] [PREVIEW_DIR]");
        std::process::exit(1);
    }
    let frames: usize = args.get(2).map_or(480, |s| s.parse().unwrap());
    let preview = args.get(3);
    // the palette k-means starts from
    let pal = encode::gaussian_pal(2350);

    // encode the frames in parallel, in order
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut encoded = vec![Vec::new(); frames];
    std::thread::scope(|s| {
        for (t, chunk) in encoded.chunks_mut(frames.div_ceil(threads)).enumerate() {
            let pal = &pal;
            let first = t * frames.div_ceil(threads);
            s.spawn(move || {
                for (i, out) in chunk.iter_mut().enumerate() {
                    let rgb = render(first + i, frames);
                    let mut enc = encode::encode_kmeans(
                        WIDTH as u32,
                        HEIGHT as u32,
                        &rgb,
                        *pal,
                        KMEANS,
                        BEAM,
                    );
                    assert!(enc.len() <= FRAME_BYTES);
                    enc.resize(FRAME_BYTES, 0);
                    *out = enc;
                }
            });
        }
    });

    let mut out = BufWriter::new(File::create(&args[1]).unwrap());
    for frame in &encoded {
        out.write_all(frame).unwrap();
    }
    out.flush().unwrap();
    println!(
        "{} frames, {} bytes, {:.1} s",
        frames,
        frames * FRAME_BYTES,
        frames as f32 / FPS
    );

    if let Some(dir) = preview {
        for n in [0, 1, frames / 2, frames - 1] {
            let size = 1028 + encode::line_stride(WIDTH as u32) * HEIGHT;
            let rgb = decode::decode(&encoded[n][..size]).unwrap();
            write_png(&format!("{dir}/frame{n:04}.png"), &rgb);
        }
    }
}

/// Frame n of frames, as 24-bit RGB.
fn render(n: usize, frames: usize) -> Vec<u8> {
    let t = n as f32 / FPS;
    let mut rgb = vec![0u8; WIDTH * HEIGHT * 3];
    // the ball bounces around the screen
    let r = 48.0;
    let bx = bounce(80.0 + 230.0 * t, r, WIDTH as f32 - r);
    let by = bounce(60.0 + 170.0 * t, r, HEIGHT as f32 - r);
    let bar = (n * 8) % WIDTH;
    let cx = 320.0 + 120.0 * (0.7 * t).cos();
    let cy = 240.0 + 90.0 * (0.9 * t).sin();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (xf, yf) = (x as f32, y as f32);
            let d = ((xf - cx).powi(2) + (yf - cy).powi(2)).sqrt();
            let v = (xf * 0.013 + t * 1.3).sin()
                + (yf * 0.021 - t * 0.9).sin()
                + ((xf + yf) * 0.009 + t * 0.7).sin()
                + (d * 0.03 - t * 2.0).sin();
            let a = v * std::f32::consts::FRAC_PI_2;
            let mut c = [
                128.0 + 100.0 * a.sin(),
                128.0 + 100.0 * (a + 2.1).sin(),
                128.0 + 100.0 * (a + 4.2).sin(),
            ];
            // the ball, lit from the upper left
            let (dx, dy) = (xf - bx, yf - by);
            let dd = (dx * dx + dy * dy).sqrt();
            if dd < r {
                let z = (1.0 - (dd / r).powi(2)).sqrt();
                let light = (0.5 * (-dx / r - dy / r) / 1.414 + 0.7 * z).max(0.0);
                c = [255.0 * light, 220.0 * light, 60.0 * light];
            }
            if x >= bar && x < bar + 6 {
                c = [255.0; 3];
            }
            let i = (y * WIDTH + x) * 3;
            for k in 0..3 {
                rgb[i + k] = c[k].clamp(0.0, 255.0) as u8;
            }
        }
    }
    // the frame number, white on black, top left
    fill(&mut rgb, 16, 16, 4 * 52 + 12, 92, [0, 0, 0]);
    let mut num = n;
    for digit in (0..4).rev() {
        draw_digit(&mut rgb, 24 + digit * 52, 24, num % 10);
        num /= 10;
    }
    // progress along the bottom
    let w = (WIDTH - 32) * (n + 1) / frames;
    fill(&mut rgb, 16, HEIGHT - 28, WIDTH - 32, 12, [0, 0, 0]);
    fill(&mut rgb, 16, HEIGHT - 28, w, 12, [255, 255, 255]);
    rgb
}

/// Position p bouncing between lo and hi.
fn bounce(p: f32, lo: f32, hi: f32) -> f32 {
    let span = hi - lo;
    let q = (p - lo).rem_euclid(2.0 * span);
    lo + if q < span { q } else { 2.0 * span - q }
}

fn fill(rgb: &mut [u8], x0: usize, y0: usize, w: usize, h: usize, c: [u8; 3]) {
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let i = (y * WIDTH + x) * 3;
            rgb[i..i + 3].copy_from_slice(&c);
        }
    }
}

/// A 40x76 seven-segment digit at x0, y0.
fn draw_digit(rgb: &mut [u8], x0: usize, y0: usize, d: usize) {
    // segments a-g, as bits 0-6
    const SEGS: [u8; 10] = [0x3f, 0x06, 0x5b, 0x4f, 0x66, 0x6d, 0x7d, 0x07, 0x7f, 0x6f];
    let (w, h, t) = (40, 76, 8);
    let rects = [
        (0, 0, w, t),
        (w - t, 0, t, h / 2),
        (w - t, h / 2, t, h / 2),
        (0, h - t, w, t),
        (0, h / 2, t, h / 2),
        (0, 0, t, h / 2),
        (0, h / 2 - t / 2, w, t),
    ];
    for (s, &(x, y, rw, rh)) in rects.iter().enumerate() {
        if SEGS[d] & (1 << s) != 0 {
            fill(rgb, x0 + x, y0 + y, rw, rh, [255, 255, 255]);
        }
    }
}

fn write_png(path: &str, rgb: &[u8]) {
    let file = File::create(path).unwrap();
    let mut encoder = png::Encoder::new(BufWriter::new(file), WIDTH as u32, HEIGHT as u32);
    encoder.set_color(png::ColorType::Rgb);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(rgb).unwrap();
}
