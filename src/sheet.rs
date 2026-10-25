//! Contact sheet layout and rendering.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use ab_glyph::{FontRef, PxScale};
use anyhow::{Context, Result};
use image::codecs::jpeg::{JpegEncoder, PixelDensity};
use image::imageops::{self, FilterType};
use image::{DynamicImage, Rgb, RgbImage};
use imageproc::drawing::{draw_hollow_rect_mut, draw_text_mut, text_size};
use imageproc::rect::Rect;

const FONT_DATA: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");

/// Sheet colours: background, labels and the thin frame around thumbnails.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub background: Rgb<u8>,
    pub text: Rgb<u8>,
    pub frame: Rgb<u8>,
}

impl Palette {
    /// White background with dark labels – for printing.
    pub const WHITE: Self = Self {
        background: Rgb([255, 255, 255]),
        text: Rgb([20, 20, 20]),
        frame: Rgb([200, 200, 200]),
    };

    /// Black background with white labels – for on-screen viewing.
    pub const BLACK: Self = Self {
        background: Rgb([0, 0, 0]),
        text: Rgb([255, 255, 255]),
        frame: Rgb([70, 70, 70]),
    };
}

/// A4 dimensions in millimetres.
const A4_MM: (f32, f32) = (210.0, 297.0);
const MM_PER_INCH: f32 = 25.4;

/// Sheet geometry in pixels.
#[derive(Debug, Clone)]
pub struct Layout {
    pub page_w: u32,
    pub page_h: u32,
    pub margin: u32,
    pub gap: u32,
    pub columns: u32,
    /// Side of the square cell a thumbnail fits into.
    pub cell: u32,
    pub label_h: u32,
    pub font_px: f32,
    pub dpi: u16,
}

impl Layout {
    pub fn a4(dpi: u16, columns: u32, landscape: bool) -> Self {
        let mm = |v: f32| (v / MM_PER_INCH * dpi as f32).round() as u32;
        let (mut page_w, mut page_h) = (mm(A4_MM.0), mm(A4_MM.1));
        if landscape {
            std::mem::swap(&mut page_w, &mut page_h);
        }
        let margin = mm(10.0);
        let gap = mm(3.0);
        let cell = (page_w - 2 * margin - (columns - 1) * gap) / columns;
        // About 7 pt – legible in print while fitting typical file names.
        let font_px = 7.0 / 72.0 * dpi as f32;
        let label_h = (font_px * 1.8).ceil() as u32;
        Self {
            page_w,
            page_h,
            margin,
            gap,
            columns,
            cell,
            label_h,
            font_px,
            dpi,
        }
    }

    /// Height available for rows (page minus margins).
    pub fn content_h(&self) -> u32 {
        self.page_h - 2 * self.margin
    }

    /// Whether a row with the tallest possible thumbnail fits on an empty page.
    pub fn fits_tallest_row(&self) -> bool {
        self.cell + self.label_h <= self.content_h()
    }

    /// Row height: the tallest thumbnail plus the label.
    fn row_h(&self, row: &[Thumb]) -> u32 {
        row_image_h(row) + self.label_h
    }
}

fn row_image_h(row: &[Thumb]) -> u32 {
    row.iter().map(|t| t.image.height()).max().unwrap_or(0)
}

/// A single contact sheet entry: a ready-to-paste thumbnail and its label.
pub struct Thumb {
    pub image: RgbImage,
    pub label: String,
}

/// Splits thumbnail rows into pages: when the next row does not fit
/// entirely on the current page, a new page is started.
pub struct Paginator {
    layout: Layout,
    rows: Vec<Vec<Thumb>>,
    used_h: u32,
}

impl Paginator {
    pub fn new(layout: Layout) -> Self {
        Self {
            layout,
            rows: Vec::new(),
            used_h: 0,
        }
    }

    /// Adds a row; returns the finished page if the row had to start a new one.
    pub fn push_row(&mut self, row: Vec<Thumb>) -> Option<Vec<Vec<Thumb>>> {
        let h = self.layout.row_h(&row);
        let mut finished = None;
        if !self.rows.is_empty() && self.used_h + self.layout.gap + h > self.layout.content_h() {
            finished = Some(std::mem::take(&mut self.rows));
        }
        self.used_h = if self.rows.is_empty() {
            h
        } else {
            self.used_h + self.layout.gap + h
        };
        self.rows.push(row);
        finished
    }

    /// Returns the last, partially filled page.
    pub fn finish(self) -> Option<Vec<Vec<Thumb>>> {
        (!self.rows.is_empty()).then_some(self.rows)
    }
}

/// Scales the image to fit within a `cell` × `cell` square.
pub fn make_thumbnail(img: &DynamicImage, cell: u32) -> RgbImage {
    // Fast pre-shrink to ~2× the target size, then an accurate filter –
    // good quality without the cost of Lanczos at full resolution.
    let pre = if img.width().max(img.height()) > cell * 2 {
        img.thumbnail(cell * 2, cell * 2)
    } else {
        img.clone()
    };
    pre.resize(cell, cell, FilterType::Lanczos3).to_rgb8()
}

pub struct Renderer {
    layout: Layout,
    font: FontRef<'static>,
    palette: Palette,
}

impl Renderer {
    pub fn new(layout: Layout, palette: Palette) -> Result<Self> {
        let font = FontRef::try_from_slice(FONT_DATA).context("invalid embedded font")?;
        Ok(Self {
            layout,
            font,
            palette,
        })
    }

    /// Renders a single page from the given thumbnail rows.
    pub fn render_page(&self, rows: &[Vec<Thumb>]) -> RgbImage {
        let l = &self.layout;
        let mut page = RgbImage::from_pixel(l.page_w, l.page_h, self.palette.background);
        let scale = PxScale::from(l.font_px);

        let mut y = l.margin;
        for row in rows {
            let img_h = row_image_h(row);
            for (col, thumb) in row.iter().enumerate() {
                let x = l.margin + col as u32 * (l.cell + l.gap);

                // Thumbnail centred horizontally and aligned to the bottom of the row,
                // so labels sit at the same height right below the photos.
                let (tw, th) = thumb.image.dimensions();
                let tx = x + (l.cell - tw) / 2;
                let ty = y + (img_h - th);
                imageops::overlay(&mut page, &thumb.image, tx as i64, ty as i64);
                draw_hollow_rect_mut(
                    &mut page,
                    Rect::at(tx as i32 - 1, ty as i32 - 1).of_size(tw + 2, th + 2),
                    self.palette.frame,
                );

                let label = fit_text(&thumb.label, l.cell, scale, &self.font);
                let (lw, _) = text_size(scale, &self.font, &label);
                let lx = x + l.cell.saturating_sub(lw) / 2;
                let ly = y + img_h + (l.label_h - l.font_px as u32) / 2;
                draw_text_mut(
                    &mut page,
                    self.palette.text,
                    lx as i32,
                    ly as i32,
                    scale,
                    &self.font,
                    &label,
                );
            }
            y += img_h + l.label_h + l.gap;
        }
        page
    }

    pub fn save_jpeg(&self, page: &RgbImage, path: &Path, quality: u8) -> Result<()> {
        let file =
            File::create(path).with_context(|| format!("cannot create {}", path.display()))?;
        let mut enc = JpegEncoder::new_with_quality(BufWriter::new(file), quality);
        enc.set_pixel_density(PixelDensity::dpi(self.layout.dpi));
        enc.encode_image(page)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }
}

/// Shortens text in the middle (with "…") so it fits within `max_w` pixels.
/// Keeps the start and end of the name, usually the number and extension.
fn fit_text(text: &str, max_w: u32, scale: PxScale, font: &FontRef) -> String {
    if text_size(scale, font, text).0 <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while keep > 1 {
        keep -= 1;
        let head = keep.div_ceil(2);
        let tail = keep - head;
        let candidate: String = chars[..head]
            .iter()
            .chain(std::iter::once(&'…'))
            .chain(&chars[chars.len() - tail..])
            .collect();
        if text_size(scale, font, &candidate).0 <= max_w {
            return candidate;
        }
    }
    "…".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thumb(w: u32, h: u32) -> Thumb {
        Thumb {
            image: RgbImage::new(w, h),
            label: String::new(),
        }
    }

    #[test]
    fn a4_at_300_dpi() {
        let l = Layout::a4(300, 5, false);
        assert_eq!((l.page_w, l.page_h), (2480, 3508));
        assert!(l.margin + l.columns * l.cell + (l.columns - 1) * l.gap <= l.page_w - l.margin);
        assert!(l.fits_tallest_row());
    }

    #[test]
    fn row_that_does_not_fit_starts_new_page() {
        let l = Layout::a4(300, 5, false);
        let row_h = l.cell + l.label_h;
        let fitting = ((l.content_h() + l.gap) / (row_h + l.gap)) as usize;
        let mut p = Paginator::new(l.clone());
        for _ in 0..fitting {
            assert!(p.push_row(vec![thumb(l.cell, l.cell)]).is_none());
        }
        let page = p
            .push_row(vec![thumb(l.cell, l.cell)])
            .expect("page should have been closed");
        assert_eq!(page.len(), fitting);
        assert_eq!(p.finish().map(|r| r.len()), Some(1));
    }

    #[test]
    fn low_rows_pack_more_per_page() {
        let l = Layout::a4(300, 5, false);
        let mut p = Paginator::new(l.clone());
        let mut rows = 0;
        while p.push_row(vec![thumb(l.cell, l.cell / 3)]).is_none() {
            rows += 1;
        }
        assert!(rows > (l.content_h() + l.gap) / (l.cell + l.label_h + l.gap));
    }

    #[test]
    fn long_names_are_shortened() {
        let font = FontRef::try_from_slice(FONT_DATA).unwrap();
        let scale = PxScale::from(30.0);
        let name = "a_very_long_file_name_of_a_holiday_photo_0001.CR2";
        let out = fit_text(name, 300, scale, &font);
        assert!(out.contains('…'));
        assert!(out.ends_with(".CR2"));
        assert!(text_size(scale, &font, &out).0 <= 300);
        assert_eq!(fit_text("IMG_1.CR2", 300, scale, &font), "IMG_1.CR2");
    }
}
