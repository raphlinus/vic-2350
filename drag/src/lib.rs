// Copyright 2026 Raph Levien
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Rectangles to drag around with the mouse.
//!
//! The app is `demo_init` and `demo_main`, called by main.sy in thread mode
//! on core 0, and drawing is by building display lists, as in rects.sy,
//! through the functions in display_list.sy.

#![no_std]

use core::ptr::addr_of_mut;

use usblog::mouse;

const SCREEN_W: i32 = 640;
const SCREEN_H: i32 = 480;
const BG_COLOR: u32 = 0x2104;
/// The line, in main.sy's v_ctr, to read the mouse at. It's in the blanking
/// before the renderer takes the list at about 42, late enough to keep
/// latency down and leaving 12 lines, or 55k cycles, to build the list.
const LATCH_LINE: u32 = 30;
/// Mouse units per pixel.
const MOUSE_DIV: i32 = 2;

/// Opaque here, with room for the two Vecs of render.sy's DisplayList,
/// which start zeroed.
#[repr(C)]
struct DisplayList([u32; 6]);

extern "C" {
    fn dvi_wait_line(v: u32);
    fn stats_init();
    fn init_alloc();
    fn render_init();
    fn dl_clear(list: *mut DisplayList);
    fn dl_solid(list: *mut DisplayList, x0y0: u32, x1y1: u32, color: u32) -> *mut u32;
    fn dl_bordered(list: *mut DisplayList, x0y0: u32, x1y1: u32, colors: u32) -> *mut u32;
    fn dl_sprite(
        list: *mut DisplayList,
        x0y0: u32,
        x1y1: u32,
        pixels: *const u16,
        stride: u32,
    ) -> *mut u32;
    fn dl_finish(list: *mut DisplayList);
    fn render_swap(list: *mut DisplayList);
}

/// A corner, as display_list.sy takes it.
fn corner(x: i32, y: i32) -> u32 {
    x as u32 | (y as u32) << 16
}

/// Half the brightness of an RGB565 color.
fn darken(color: u32) -> u32 {
    (color >> 1) & 0x7bef
}

/// Append a sprite of w by h pixels at x, y, clipped to the screen.
unsafe fn add_sprite(list: *mut DisplayList, x: i32, y: i32, w: i32, h: i32, pixels: *const u16) {
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(SCREEN_W);
    let y1 = (y + h).min(SCREEN_H);
    if x0 < x1 && y0 < y1 {
        let start = pixels.add(((y0 - y) * w + x0 - x) as usize);
        dl_sprite(list, corner(x0, y0), corner(x1, y1), start, w as u32 * 2);
    }
}

#[derive(Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: u32,
}

impl Rect {
    const fn new(x: i32, y: i32, w: i32, h: i32, color: u32) -> Self {
        Self { x, y, w, h, color }
    }

    fn contains(&self, x: i32, y: i32) -> bool {
        (self.x..self.x + self.w).contains(&x) && (self.y..self.y + self.h).contains(&y)
    }
}

/// In draw order, so the last is on top.
static mut RECTS: [Rect; 16] = [
    Rect::new(194, 94, 128, 134, 0xd1c7),
    Rect::new(500, 355, 60, 100, 0x25ea),
    Rect::new(161, 319, 110, 94, 0x901e),
    Rect::new(398, 237, 134, 118, 0xb52a),
    Rect::new(4, 339, 48, 64, 0x2537),
    Rect::new(85, 362, 122, 66, 0xc90f),
    Rect::new(110, 205, 34, 30, 0x4ea3),
    Rect::new(214, 26, 86, 174, 0x527e),
    Rect::new(276, 172, 40, 34, 0xca20),
    Rect::new(252, 51, 86, 146, 0x57d7),
    Rect::new(521, 97, 84, 130, 0xc1da),
    Rect::new(276, 303, 62, 92, 0x95c4),
    Rect::new(51, 21, 136, 112, 0x5479),
    Rect::new(395, 37, 122, 76, 0xa989),
    Rect::new(464, 65, 28, 34, 0x570c),
    Rect::new(233, 312, 86, 112, 0x82d9),
];

const CURSOR_W: usize = 12;
const CURSOR_H: usize = 19;
const CURSOR_ART: [&[u8; CURSOR_W]; CURSOR_H] = [
    b"X...........",
    b"XX..........",
    b"X#X.........",
    b"X##X........",
    b"X###X.......",
    b"X####X......",
    b"X#####X.....",
    b"X######X....",
    b"X#######X...",
    b"X########X..",
    b"X#########X.",
    b"X######XXXXX",
    b"X###X##X....",
    b"X##XX##X....",
    b"X#X..X##X...",
    b"XX...X##X...",
    b"X.....X##X..",
    b"......X##X..",
    b".......XX...",
];

const fn cursor_pixels() -> [u16; CURSOR_W * CURSOR_H] {
    let mut pixels = [0; CURSOR_W * CURSOR_H];
    let mut i = 0;
    while i < pixels.len() {
        pixels[i] = match CURSOR_ART[i / CURSOR_W][i % CURSOR_W] {
            b'X' => 0x0001,
            b'#' => 0xffff,
            _ => 0,
        };
        i += 1;
    }
    pixels
}

/// In RAM, for the render interrupt. The tip is the top left corner.
#[link_section = ".data"]
static CURSOR: [u16; CURSOR_W * CURSOR_H] = cursor_pixels();

static mut LISTS: [DisplayList; 2] = [const { DisplayList([0; 6]) }; 2];

/// What's being dragged: the rectangle is the top one, and this is where
/// it was grabbed, from its corner.
struct Grab {
    dx: i32,
    dy: i32,
}

struct App {
    rects: &'static mut [Rect],
    // The cursor, in mouse units, so that slow movement isn't lost.
    x: i32,
    y: i32,
    last: mouse::Mouse,
    grab: Option<Grab>,
}

impl App {
    fn update(&mut self) {
        let mouse = mouse::read();
        let (max_x, max_y) = ((SCREEN_W - 1) * MOUSE_DIV, (SCREEN_H - 1) * MOUSE_DIV);
        self.x = (self.x + mouse.x.wrapping_sub(self.last.x)).clamp(0, max_x);
        self.y = (self.y + mouse.y.wrapping_sub(self.last.y)).clamp(0, max_y);
        let held = mouse.buttons & 1 != 0;
        let was_held = self.last.buttons & 1 != 0;
        self.last = mouse;
        if held && !was_held {
            self.grab();
        } else if !held {
            self.grab = None;
        }
        let (x, y) = self.cursor();
        if let (Some(grab), Some(rect)) = (&self.grab, self.rects.last_mut()) {
            rect.x = (x - grab.dx).clamp(0, SCREEN_W - rect.w);
            rect.y = (y - grab.dy).clamp(0, SCREEN_H - rect.h);
        }
    }

    /// The cursor's pixel.
    fn cursor(&self) -> (i32, i32) {
        (self.x / MOUSE_DIV, self.y / MOUSE_DIV)
    }

    /// Pick up the top rectangle under the cursor, which goes on top.
    fn grab(&mut self) {
        let (x, y) = self.cursor();
        let hit = self.rects.iter().rposition(|r| r.contains(x, y));
        if let Some(ix) = hit {
            let rect = self.rects[ix];
            self.rects[ix..].rotate_left(1);
            self.grab = Some(Grab {
                dx: x - rect.x,
                dy: y - rect.y,
            });
        }
    }

    unsafe fn draw(&self, list: *mut DisplayList) {
        dl_clear(list);
        dl_solid(list, 0, corner(SCREEN_W, SCREEN_H), BG_COLOR);
        for r in self.rects.iter() {
            let colors = r.color | darken(r.color) << 16;
            dl_bordered(list, corner(r.x, r.y), corner(r.x + r.w, r.y + r.h), colors);
        }
        let (w, h) = (CURSOR_W as i32, CURSOR_H as i32);
        let (x, y) = self.cursor();
        add_sprite(list, x, y, w, h, CURSOR.as_ptr());
        dl_finish(list);
    }
}

#[no_mangle]
pub unsafe extern "C" fn demo_init() {
    stats_init();
    init_alloc();
    render_init();
}

#[no_mangle]
pub unsafe extern "C" fn demo_main() {
    let mut app = App {
        rects: &mut *addr_of_mut!(RECTS),
        x: SCREEN_W / 2 * MOUSE_DIV,
        y: SCREEN_H / 2 * MOUSE_DIV,
        last: mouse::read(),
        grab: None,
    };
    let mut ix = 0;
    loop {
        dvi_wait_line(LATCH_LINE);
        app.update();
        // Built alternately: one is rendered while the other is built.
        let list = addr_of_mut!(LISTS[ix]);
        app.draw(list);
        render_swap(list);
        ix ^= 1;
    }
}
