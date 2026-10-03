//! The menu, drawn: the screen at the display's native resolution, read
//! from its EDID, a font that stays readable from 800 lines to 4K, and the
//! entries as rows.
//! Only while the menu is up — the firmware's mode comes back before an
//! image starts, so what Linux inherits is what the firmware chose.
//!
//! Drawn into a buffer the size of the screen and copied in one transfer:
//! no flicker as the selection moves. Where there is no graphics output —
//! a serial console — the menu stays text; see `app::menu`.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use hideboot_core::{Entry, choose_mode, edid_native_resolution};
use noto_sans_mono_bitmap::{FontWeight, RasterHeight, get_raster, get_raster_width};
use uefi::boot::ScopedProtocol;
use uefi::proto::console::gop::{BltOp, BltPixel, BltRegion, GraphicsOutput, Mode};

/// hideOS's colours: Dracula's.
const BACKGROUND: Rgb = Rgb(0x28, 0x2a, 0x36);
const SELECTION: Rgb = Rgb(0x44, 0x47, 0x5a);
const FOREGROUND: Rgb = Rgb(0xf8, 0xf8, 0xf2);
const COMMENT: Rgb = Rgb(0x62, 0x72, 0xa4);
const PURPLE: Rgb = Rgb(0xbd, 0x93, 0xf9);
const PINK: Rgb = Rgb(0xff, 0x79, 0xc6);
const CYAN: Rgb = Rgb(0x8b, 0xe9, 0xfd);
const YELLOW: Rgb = Rgb(0xf1, 0xfa, 0x8c);
const RED: Rgb = Rgb(0xff, 0x55, 0x55);

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy)]
struct Rgb(u8, u8, u8);

/// What a row says about its entry, and in which colour.
pub struct Row<'a> {
    pub entry: &'a Entry,
    pub recovery: bool,
}

pub struct Screen {
    gop: ScopedProtocol<GraphicsOutput>,
    /// The mode the firmware was in, to go back to.
    original: Option<Mode>,
    width: usize,
    height: usize,
    buffer: Vec<BltPixel>,
    font: Font,
}

impl Screen {
    /// The screen at its largest mode, or `None` with no graphics output.
    pub fn open() -> Option<Screen> {
        let (mut gop, edid) = crate::sys::graphics_output()?;
        let current = gop.current_mode_info();
        let modes: Vec<Mode> = gop.modes().collect();
        let original = modes.iter().copied().find(|m| *m.info() == current);
        let resolutions: Vec<(usize, usize)> =
            modes.iter().map(|m| m.info().resolution()).collect();
        let native = edid.as_deref().and_then(edid_native_resolution);
        if let Some(mode) = choose_mode(&resolutions, native).and_then(|i| modes.get(i))
            && *mode.info() != current
        {
            // A mode the firmware lists but cannot set leaves the one it
            // was in: the menu is drawn there instead.
            let _ = gop.set_mode(mode);
        }
        let (width, height) = gop.current_mode_info().resolution();
        if width == 0 || height == 0 {
            return None;
        }
        uefi::println!(
            "hideBoot: the menu at {width}x{height}; the display says {}",
            match native {
                Some((w, h)) => format!("{w}x{h}"),
                None => String::from("nothing"),
            }
        );
        Some(Screen {
            gop,
            original,
            width,
            height,
            buffer: vec![BltPixel::new(0, 0, 0); width.checked_mul(height)?],
            font: Font::for_height(height),
        })
    }

    /// Puts the firmware's mode back, and the screen black.
    pub fn close(mut self) {
        if let Some(mode) = self.original
            && *mode.info() != self.gop.current_mode_info()
        {
            let _ = self.gop.set_mode(&mode);
        }
        let (width, height) = self.gop.current_mode_info().resolution();
        let _ = self.gop.blt(BltOp::VideoFill {
            color: BltPixel::new(0, 0, 0),
            dest: (0, 0),
            dims: (width, height),
        });
    }

    pub fn draw(&mut self, rows: &[Row<'_>], selected: usize) {
        self.fill(0, 0, self.width, self.height, BACKGROUND);
        let line = self.font.line_height();
        let char_w = self.font.char_width();

        // A column of about 76 characters, centred, a third of the way down.
        let column = (char_w * 76).min(self.width.saturating_sub(char_w * 4));
        let left = (self.width.saturating_sub(column)) / 2;
        let mut y = self.height / 4;

        self.text(left, y, "hideBoot", FontWeight::Bold, PURPLE);
        self.text(left + char_w * 9, y, VERSION, FontWeight::Regular, COMMENT);
        y += line + line / 2;
        self.text(
            left,
            y,
            "Choose what starts.",
            FontWeight::Regular,
            COMMENT,
        );
        y += line * 2;

        let row_h = line + line * 3 / 4;
        let pad = (row_h - line) / 2;
        for (i, row) in rows.iter().enumerate() {
            if i == selected {
                self.fill(left, y, column, row_h, SELECTION);
                self.fill(left, y, (char_w / 3).max(2), row_h, PINK);
            }
            let number = format!("{}", i + 1);
            self.text(left + char_w * 2, y + pad, &number, FontWeight::Regular, COMMENT);
            let weight = if i == selected {
                FontWeight::Bold
            } else {
                FontWeight::Regular
            };
            self.text(left + char_w * 5, y + pad, &row.entry.id, weight, FOREGROUND);
            if let Some((note, colour)) = note(row) {
                let x = (left + column).saturating_sub(char_w * (note.len() + 2));
                self.text(x, y + pad, &note, FontWeight::Regular, colour);
            }
            y += row_h;
        }

        y += line * 2;
        self.text(
            left,
            y,
            "Up and Down to choose, Enter to start, or the entry's number.",
            FontWeight::Regular,
            COMMENT,
        );
        self.flush();
    }

    fn flush(&mut self) {
        let _ = self.gop.blt(BltOp::BufferToVideo {
            buffer: &self.buffer,
            src: BltRegion::Full,
            dest: (0, 0),
            dims: (self.width, self.height),
        });
    }

    fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, colour: Rgb) {
        let pixel = BltPixel::new(colour.0, colour.1, colour.2);
        for row in y..(y + h).min(self.height) {
            let start = row * self.width + x.min(self.width);
            let end = row * self.width + (x + w).min(self.width);
            if let Some(span) = self.buffer.get_mut(start..end) {
                span.fill(pixel);
            }
        }
    }

    /// Draws `text` with its top left at (`x`, `y`), blending the font's
    /// grey levels over what is there.
    fn text(&mut self, x: usize, y: usize, text: &str, weight: FontWeight, colour: Rgb) {
        let scale = self.font.scale;
        let advance = self.font.char_width();
        for (n, c) in text.chars().enumerate() {
            let Some(glyph) = get_raster(c, weight, self.font.size)
                .or_else(|| get_raster('?', weight, self.font.size))
            else {
                continue;
            };
            let origin_x = x + n * advance;
            for (gy, row) in glyph.raster().iter().enumerate() {
                for (gx, &level) in row.iter().enumerate() {
                    if level == 0 {
                        continue;
                    }
                    for sy in 0..scale {
                        let py = y + gy * scale + sy;
                        if py >= self.height {
                            continue;
                        }
                        for sx in 0..scale {
                            let px = origin_x + gx * scale + sx;
                            if px >= self.width {
                                continue;
                            }
                            if let Some(pixel) = self.buffer.get_mut(py * self.width + px) {
                                *pixel = blend(*pixel, colour, level);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The note on the right of a row: what the boot counter says.
fn note(row: &Row<'_>) -> Option<(String, Rgb)> {
    if row.recovery {
        return Some((String::from("recovery"), CYAN));
    }
    let counter = row.entry.counter?;
    if counter.left == 0 {
        return Some((String::from("failed to start"), RED));
    }
    let tries = if counter.left == 1 { "try" } else { "tries" };
    Some((format!("being tried, {} {tries} left", counter.left), YELLOW))
}

fn blend(under: BltPixel, over: Rgb, level: u8) -> BltPixel {
    let mix = |a: u8, b: u8| -> u8 {
        let a = u32::from(a);
        let b = u32::from(b);
        let l = u32::from(level);
        u8::try_from((a * (255 - l) + b * l) / 255).unwrap_or(u8::MAX)
    };
    BltPixel::new(
        mix(under.red, over.0),
        mix(under.green, over.1),
        mix(under.blue, over.2),
    )
}

/// One of the font's rasterised heights, scaled by whole pixels: text
/// about a fortieth of the screen tall, from 20 pixels at 800 lines to 54
/// at 2160.
struct Font {
    size: RasterHeight,
    scale: usize,
}

impl Font {
    fn for_height(height: usize) -> Font {
        let target = (height / 40).max(16);
        let mut best = Font {
            size: RasterHeight::Size16,
            scale: 1,
        };
        let mut best_error = usize::MAX;
        for size in [
            RasterHeight::Size16,
            RasterHeight::Size20,
            RasterHeight::Size24,
            RasterHeight::Size32,
        ] {
            for scale in 1..=3 {
                let h = size.val() * scale;
                let error = h.abs_diff(target);
                if error < best_error {
                    best_error = error;
                    best = Font { size, scale };
                }
            }
        }
        best
    }

    fn line_height(&self) -> usize {
        self.size.val() * self.scale
    }

    fn char_width(&self) -> usize {
        get_raster_width(FontWeight::Regular, self.size) * self.scale
    }
}
