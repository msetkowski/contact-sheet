//! Contact sheet layout and rendering.

use std::fs::File;
use std::io::{BufWriter, Write};
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

const MM_PER_INCH: f32 = 25.4;
const POINTS_PER_INCH: f32 = 72.0;

/// Paper size in millimetres, portrait (width < height).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paper {
    pub width_mm: f32,
    pub height_mm: f32,
}

impl Paper {
    pub const A4: Self = Self {
        width_mm: 210.0,
        height_mm: 297.0,
    };
    pub const A3: Self = Self {
        width_mm: 297.0,
        height_mm: 420.0,
    };
    /// US Letter, 8.5 × 11 in.
    pub const LETTER: Self = Self {
        width_mm: 215.9,
        height_mm: 279.4,
    };
}

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
    /// Exact page size in points (1/72 in), unaffected by pixel rounding.
    pub page_size_pt: (f32, f32),
}

impl Layout {
    pub fn new(paper: Paper, dpi: u16, columns: u32, landscape: bool) -> Self {
        let mm = |v: f32| (v / MM_PER_INCH * dpi as f32).round() as u32;
        let pt = |v: f32| v / MM_PER_INCH * POINTS_PER_INCH;
        let (mut page_w, mut page_h) = (mm(paper.width_mm), mm(paper.height_mm));
        let mut page_size_pt = (pt(paper.width_mm), pt(paper.height_mm));
        if landscape {
            std::mem::swap(&mut page_w, &mut page_h);
            page_size_pt = (page_size_pt.1, page_size_pt.0);
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
            page_size_pt,
        }
    }

    /// Thumbnails only: no file names below them, so rows sit closer together.
    pub fn without_labels(mut self) -> Self {
        self.label_h = 0;
        self
    }

    fn has_labels(&self) -> bool {
        self.label_h > 0
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

                if !l.has_labels() {
                    continue;
                }
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
        self.write_jpeg(page, BufWriter::new(file), quality)
            .with_context(|| format!("failed to write {}", path.display()))
    }

    /// Encodes a page as JPEG into memory (for embedding in a PDF).
    pub fn encode_jpeg(&self, page: &RgbImage, quality: u8) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.write_jpeg(page, &mut buf, quality)
            .context("failed to encode page")?;
        Ok(buf)
    }

    fn write_jpeg(&self, page: &RgbImage, out: impl Write, quality: u8) -> Result<()> {
        let mut enc = JpegEncoder::new_with_quality(out, quality);
        enc.set_pixel_density(PixelDensity::dpi(self.layout.dpi));
        enc.encode_image(page)?;
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
        let l = Layout::new(Paper::A4, 300, 5, false);
        assert_eq!((l.page_w, l.page_h), (2480, 3508));
        assert!(l.margin + l.columns * l.cell + (l.columns - 1) * l.gap <= l.page_w - l.margin);
        assert!(l.fits_tallest_row());
    }

    #[test]
    fn a3_and_letter_sizes() {
        let a3 = Layout::new(Paper::A3, 300, 5, false);
        assert_eq!((a3.page_w, a3.page_h), (3508, 4961));
        let letter = Layout::new(Paper::LETTER, 300, 5, false);
        assert_eq!((letter.page_w, letter.page_h), (2550, 3300));
        let (w, h) = letter.page_size_pt;
        assert!((w - 612.0).abs() < 0.01 && (h - 792.0).abs() < 0.01);
    }

    #[test]
    fn landscape_swaps_pixels_and_points() {
        let l = Layout::new(Paper::A3, 150, 5, true);
        assert!(l.page_w > l.page_h);
        assert!(l.page_size_pt.0 > l.page_size_pt.1);
    }

    #[test]
    fn page_without_labels_shows_only_thumbnails() {
        let l = Layout::new(Paper::A4, 150, 5, false).without_labels();
        let renderer = Renderer::new(l.clone(), Palette::WHITE).unwrap();
        let mut image = RgbImage::from_pixel(l.cell, l.cell / 2, Rgb([255, 0, 0]));
        image.put_pixel(0, 0, Rgb([0, 0, 255]));
        let row = vec![Thumb {
            image,
            label: "IMG_0001.CR2".to_string(),
        }];
        let page = renderer.render_page(&[row]);

        // Below the thumbnail and its 1 px frame there is only background.
        let below = l.margin + l.cell / 2 + 1;
        let x_range = l.margin - 1..l.margin + l.cell + 1;
        for y in below..below + 2 * l.font_px as u32 {
            for x in x_range.clone() {
                assert_eq!(
                    *page.get_pixel(x, y),
                    Palette::WHITE.background,
                    "({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn rows_without_labels_pack_more_per_page() {
        let count_rows = |l: Layout| {
            let mut p = Paginator::new(l.clone());
            let mut rows = 0;
            while p.push_row(vec![thumb(l.cell, l.cell)]).is_none() {
                rows += 1;
            }
            rows
        };
        let l = Layout::new(Paper::A4, 300, 8, false);
        assert!(count_rows(l.clone().without_labels()) > count_rows(l));
    }

    #[test]
    fn row_that_does_not_fit_starts_new_page() {
        let l = Layout::new(Paper::A4, 300, 5, false);
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
        let l = Layout::new(Paper::A4, 300, 5, false);
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
