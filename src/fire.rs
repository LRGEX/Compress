// Procedural fire effect — doom-fire algorithm.
// Based on: https://notryanb.github.io/rust-doom-fire-fx.html
// The fire buffer is indexed [0..W*H]. Row 0 = TOP, row H-1 = BOTTOM.
// Bottom row is set to max intensity (36). Fire spreads UPWARD each frame.

use rand::Rng;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

const FIRE_WIDTH: usize = 80;
const FIRE_HEIGHT: usize = 80;

// Doom-fire palette: 37 colors, index 0 = transparent black, 36 = white.
const PALETTE: [(u8, u8, u8); 37] = [
    (0x07, 0x07, 0x07),
    (0x1F, 0x07, 0x07),
    (0x2F, 0x0F, 0x07),
    (0x47, 0x0F, 0x07),
    (0x57, 0x17, 0x07),
    (0x67, 0x1F, 0x07),
    (0x77, 0x1F, 0x07),
    (0x8F, 0x27, 0x07),
    (0x9F, 0x2F, 0x07),
    (0xAF, 0x3F, 0x07),
    (0xBF, 0x47, 0x07),
    (0xC7, 0x47, 0x07),
    (0xDF, 0x4F, 0x07),
    (0xDF, 0x57, 0x07),
    (0xDF, 0x57, 0x07),
    (0xD7, 0x5F, 0x07),
    (0xD7, 0x5F, 0x07),
    (0xD7, 0x67, 0x0F),
    (0xCF, 0x6F, 0x0F),
    (0xCF, 0x77, 0x0F),
    (0xCF, 0x7F, 0x0F),
    (0xCF, 0x87, 0x17),
    (0xC7, 0x87, 0x17),
    (0xC7, 0x8F, 0x17),
    (0xC7, 0x97, 0x1F),
    (0xBF, 0x9F, 0x1F),
    (0xBF, 0x9F, 0x1F),
    (0xBF, 0xA7, 0x27),
    (0xBF, 0xA7, 0x27),
    (0xBF, 0xAF, 0x2F),
    (0xB7, 0xAF, 0x2F),
    (0xB7, 0xB7, 0x2F),
    (0xB7, 0xB7, 0x37),
    (0xCF, 0xCF, 0x6F),
    (0xDF, 0xDF, 0x9F),
    (0xEF, 0xEF, 0xC7),
    (0xFF, 0xFF, 0xFF),
];

pub struct Fire {
    pixels: [u8; FIRE_WIDTH * FIRE_HEIGHT],
    buffer: SharedPixelBuffer<Rgba8Pixel>,
}

impl Fire {
    pub fn new() -> Self {
        let mut pixels = [0u8; FIRE_WIDTH * FIRE_HEIGHT];
        // Bottom row = max intensity (fire source).
        for x in 0..FIRE_WIDTH {
            pixels[(FIRE_HEIGHT - 1) * FIRE_WIDTH + x] = 36;
        }
        Self {
            pixels,
            buffer: SharedPixelBuffer::new(FIRE_WIDTH as u32, FIRE_HEIGHT as u32),
        }
    }

    /// Advance the fire simulation by one frame (doom-fire spread algorithm).
    pub fn tick(&mut self) {
        let mut rng = rand::thread_rng();

        // Iterate column by column, bottom to top.
        for x in 0..FIRE_WIDTH {
            for y in (1..FIRE_HEIGHT).rev() {
                let src = y * FIRE_WIDTH + x;
                self.spread_fire(src, &mut rng);
            }
        }

        // Render to pixel buffer. The buffer is bottom-up in the algorithm
        // but top-down in display, so we DON'T flip — the doom algorithm
        // already writes upward correctly (row 0 = top of fire = coldest).
        let buf = self.buffer.make_mut_slice();
        for i in 0..FIRE_WIDTH * FIRE_HEIGHT {
            let idx = self.pixels[i] as usize;
            let (r, g, b) = PALETTE[idx.min(36)];
            let pixel = &mut buf[i];
            pixel.r = r;
            pixel.g = g;
            pixel.b = b;
            // Transparent for the darkest pixels (index 0), opaque for everything else.
            pixel.a = if idx == 0 { 0 } else { 255 };
        }
    }

    fn spread_fire(&mut self, src: usize, rng: &mut impl Rng) {
        let pixel = self.pixels[src];
        if pixel == 0 {
            // Cold pixel — the pixel above it goes cold too.
            if src >= FIRE_WIDTH {
                self.pixels[src - FIRE_WIDTH] = 0;
            }
        } else {
            // Random horizontal offset (0..3) — creates the flickering.
            let rand_val: u32 = rng.gen_range(0..4);
            // Decay: subtract 0 or 1 based on randomness.
            let decay = (rand_val & 1) as u8;
            // Destination: the pixel ABOVE the source, shifted horizontally.
            let dst_x = (src % FIRE_WIDTH) as isize - rand_val as isize + 1;
            let dst_x = dst_x.rem_euclid(FIRE_WIDTH as isize) as usize;
            let dst_y = (src / FIRE_WIDTH) - 1; // one row up
            let dst = dst_y * FIRE_WIDTH + dst_x;
            self.pixels[dst] = pixel.saturating_sub(decay);
        }
    }

    /// Get the current frame as a Slint Image.
    pub fn image(&self) -> Image {
        Image::from_rgba8(self.buffer.clone())
    }
}
