//! Draw a captured terminal screen as a PNG.
//!
//! Every cell is a 16x24 pixel block: the 8x8 bitmap glyph from `font8x8`
//! doubled and vertically centered (box and block glyphs are stretched to
//! the full height so borders join), in the cell's colors. Bold uses the
//! bright color, dim halves the color, inverse swaps foreground and
//! background, underline draws a line.
//! No system fonts are involved, so the image is identical on every machine
//! and in CI; glyphs the bitmap font lacks are drawn as a hollow box.

use std::io::BufWriter;
use std::path::Path;

use font8x8::UnicodeFonts;

const SCALE: usize = 2;
const CELL_WIDTH: usize = 8 * SCALE;
/// Terminal cells are taller than wide; the glyph sits in the middle.
const CELL_HEIGHT: usize = 12 * SCALE;
const GLYPH_TOP: usize = (CELL_HEIGHT - 8 * SCALE) / 2;
/// Vertical scale of box-drawing and block glyphs: the whole cell.
const LINE_SCALE_Y: usize = CELL_HEIGHT / 8;
const DEFAULT_FG: [u8; 3] = [212, 212, 212];
const DEFAULT_BG: [u8; 3] = [24, 24, 28];
/// The 16 ANSI colors (Visual Studio Code's dark palette).
const PALETTE: [[u8; 3]; 16] = [
    [0, 0, 0],
    [205, 49, 49],
    [13, 188, 121],
    [229, 229, 16],
    [36, 114, 200],
    [188, 63, 188],
    [17, 168, 205],
    [229, 229, 229],
    [102, 102, 102],
    [241, 76, 76],
    [35, 209, 139],
    [245, 245, 67],
    [59, 142, 234],
    [214, 112, 214],
    [41, 184, 219],
    [255, 255, 255],
];

pub fn write_png(screen: &vt100::Screen, path: &Path) -> std::io::Result<()> {
    let (rows, cols) = screen.size();
    let width = cols as usize * CELL_WIDTH;
    let height = rows as usize * CELL_HEIGHT;
    let mut image = Image {
        width,
        pixels: vec![0; width * height * 3],
    };
    image.fill(0, 0, width, height, DEFAULT_BG);

    for row in 0..rows {
        for col in 0..cols {
            let cell = screen.cell(row, col).expect("cell inside the screen");
            if cell.is_wide_continuation() {
                continue;
            }
            let mut fg = color(cell.fgcolor(), cell.bold(), DEFAULT_FG);
            let mut bg = color(cell.bgcolor(), false, DEFAULT_BG);
            if cell.dim() {
                fg = fg.map(|channel| channel / 2);
            }
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }
            let span = if cell.is_wide() { 2 } else { 1 };
            let (x, y) = (col as usize * CELL_WIDTH, row as usize * CELL_HEIGHT);
            image.fill(x, y, span * CELL_WIDTH, CELL_HEIGHT, bg);
            if let Some(ch) = cell
                .contents()
                .chars()
                .next()
                .filter(|c| !c.is_whitespace())
            {
                let centered = x + (span * CELL_WIDTH - CELL_WIDTH) / 2;
                if is_line_drawing(ch) {
                    image.glyph(centered, y, &glyph(ch), fg, LINE_SCALE_Y);
                } else {
                    image.glyph(centered, y + GLYPH_TOP, &glyph(ch), fg, SCALE);
                }
            }
            if cell.underline() {
                image.fill(x, y + CELL_HEIGHT - 2, span * CELL_WIDTH, 1, fg);
            }
        }
    }

    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
    writer
        .write_image_data(&image.pixels)
        .map_err(std::io::Error::other)?;
    Ok(())
}

struct Image {
    width: usize,
    pixels: Vec<u8>,
}

impl Image {
    fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, color: [u8; 3]) {
        for row in y..y + h {
            for column in x..x + w {
                let start = (row * self.width + column) * 3;
                self.pixels[start..start + 3].copy_from_slice(&color);
            }
        }
    }

    /// Bit 0 of each row is the leftmost pixel, as `font8x8` stores it.
    fn glyph(&mut self, x: usize, y: usize, glyph: &[u8; 8], color: [u8; 3], scale_y: usize) {
        for (dy, bits) in glyph.iter().enumerate() {
            for dx in 0..8 {
                if bits & (1 << dx) != 0 {
                    self.fill(x + dx * SCALE, y + dy * scale_y, SCALE, scale_y, color);
                }
            }
        }
    }
}

fn color(color: vt100::Color, bold: bool, default: [u8; 3]) -> [u8; 3] {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Idx(index) if index < 8 && bold => PALETTE[index as usize + 8],
        vt100::Color::Idx(index) if index < 16 => PALETTE[index as usize],
        vt100::Color::Idx(index) if index < 232 => {
            let index = index - 16;
            let level = |value: u8| if value == 0 { 0 } else { 55 + 40 * value };
            [level(index / 36), level((index / 6) % 6), level(index % 6)]
        }
        vt100::Color::Idx(index) => {
            let gray = 8 + 10 * (index - 232);
            [gray, gray, gray]
        }
        vt100::Color::Rgb(r, g, b) => [r, g, b],
    }
}

/// Box drawing (U+2500..U+257F) and block elements (U+2580..U+259F).
fn is_line_drawing(ch: char) -> bool {
    ('\u{2500}'..='\u{259F}').contains(&ch)
}

fn glyph(ch: char) -> [u8; 8] {
    use font8x8::{BASIC_FONTS, BLOCK_FONTS, BOX_FONTS, HIRAGANA_FONTS, LATIN_FONTS, MISC_FONTS};
    BASIC_FONTS
        .get(ch)
        .or_else(|| LATIN_FONTS.get(ch))
        .or_else(|| BOX_FONTS.get(ch))
        .or_else(|| BLOCK_FONTS.get(ch))
        .or_else(|| HIRAGANA_FONTS.get(ch))
        .or_else(|| MISC_FONTS.get(ch))
        .or_else(|| extra_glyph(ch))
        .unwrap_or_else(|| bitmap(TOFU))
}

/// Glyphs the TUI uses that the bitmap font does not have.
fn extra_glyph(ch: char) -> Option<[u8; 8]> {
    let rows = match ch {
        '↑' => [
            "...#....", "..###...", ".#.#.#..", "...#....", "...#....", "...#....", "...#....",
            "........",
        ],
        '↓' => [
            "........", "...#....", "...#....", "...#....", "...#....", ".#.#.#..", "..###...",
            "...#....",
        ],
        '▸' | '▶' => [
            "........", "..#.....", "..##....", "..###...", "..####..", "..###...", "..##....",
            "..#.....",
        ],
        '…' => [
            "........", "........", "........", "........", "........", "........", "#..#..#.",
            "........",
        ],
        '●' => [
            "........", "..####..", ".######.", ".######.", ".######.", ".######.", "..####..",
            "........",
        ],
        _ => return None,
    };
    Some(bitmap(rows))
}

const TOFU: [&str; 8] = [
    "########", "#......#", "#......#", "#......#", "#......#", "#......#", "#......#", "########",
];

fn bitmap(rows: [&str; 8]) -> [u8; 8] {
    rows.map(|row| {
        row.chars()
            .enumerate()
            .filter(|(_, c)| *c == '#')
            .fold(0u8, |bits, (index, _)| bits | (1 << index))
    })
}
