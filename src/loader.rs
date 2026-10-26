//! Loading source photos (Canon, Nikon, Fujifilm and Hasselblad RAW, DNG, TIFF, JPEG) into a
//! `DynamicImage`.

use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow, bail};
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbImage};
use rawler::decoders::{Decoder, RawDecodeParams};
use rawler::formats::tiff::GenericTiffReader;
use rawler::formats::tiff::reader::TiffReader;
use rawler::imgop::develop::RawDevelop;
use rawler::rawsource::RawSource;
use rawler::tags::TiffCommonTag;

/// Kind of supported input file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// Canon (CR2, CR3, CRW), Nikon (NEF, NRW), Fujifilm (RAF) and Hasselblad (3FR, FFF)
    /// RAW, and DNG – decoded by `rawler`.
    Raw,
    /// TIFF (including uncompressed, 8/16 bit) and JPEG – decoded by `image`.
    Image,
}

/// Extensions of the RAW formats (and DNG), lowercase.
const RAW_EXTENSIONS: &[&str] = &[
    "cr2", "cr3", "crw", "nef", "nrw", "raf", "3fr", "fff", "dng",
];
const JPEG_EXTENSIONS: &[&str] = &["jpg", "jpeg"];
const TIFF_EXTENSIONS: &[&str] = &["tif", "tiff"];

/// Values accepted by `--only`, for help and error messages.
pub const FORMAT_FILTERS: &str = "raw, cr2, cr3, crw, nef, nrw, raf, 3fr, fff, dng, jpg, tif";

/// Detects the file kind from its extension (case-insensitive).
pub fn source_kind(path: &Path) -> Option<SourceKind> {
    let ext = lowercase_extension(path)?;
    if RAW_EXTENSIONS.contains(&ext.as_str()) {
        Some(SourceKind::Raw)
    } else if JPEG_EXTENSIONS.contains(&ext.as_str()) || TIFF_EXTENSIONS.contains(&ext.as_str()) {
        Some(SourceKind::Image)
    } else {
        None
    }
}

pub fn lowercase_extension(path: &Path) -> Option<String> {
    Some(path.extension()?.to_str()?.to_ascii_lowercase())
}

/// The extensions a `--only` value stands for (case-insensitive): `raw` for
/// every RAW format including DNG, `jpg`/`jpeg` and `tif`/`tiff` for both
/// spellings, or a single RAW extension such as `cr3`.
pub fn format_filter(name: &str) -> Option<&'static [&'static str]> {
    let name = name.trim().trim_start_matches('.').to_ascii_lowercase();
    match name.as_str() {
        "raw" => Some(RAW_EXTENSIONS),
        "jpg" | "jpeg" => Some(JPEG_EXTENSIONS),
        "tif" | "tiff" => Some(TIFF_EXTENSIONS),
        ext => RAW_EXTENSIONS
            .iter()
            .position(|&raw| raw == ext)
            .map(|i| &RAW_EXTENSIONS[i..=i]),
    }
}

/// Human-readable format name of a supported file, based on its extension.
pub fn format_name(path: &Path) -> &'static str {
    let ext = lowercase_extension(path).unwrap_or_default();
    match ext.as_str() {
        "cr2" => "Canon RAW (CR2)",
        "cr3" => "Canon RAW (CR3)",
        "crw" => "Canon RAW (CRW)",
        "nef" => "Nikon RAW (NEF)",
        "nrw" => "Nikon RAW (NRW)",
        "raf" => "Fujifilm RAW (RAF)",
        "3fr" => "Hasselblad RAW (3FR)",
        "fff" => "Hasselblad RAW (FFF)",
        "dng" => "DNG",
        "tif" | "tiff" => "TIFF",
        "jpg" | "jpeg" => "JPEG",
        _ => "unknown",
    }
}

/// Loads a photo and returns it in display orientation.
///
/// `min_size` is the smallest acceptable longer edge – for RAW files an
/// embedded JPEG preview smaller than this is skipped in favour of
/// fully developing the sensor data.
pub fn load(path: &Path, kind: SourceKind, min_size: u32) -> Result<DynamicImage> {
    match kind {
        SourceKind::Raw => load_raw(path, min_size),
        SourceKind::Image => load_image(path, min_size),
    }
}

/// Returns the photo's pixel dimensions (width, height) in display orientation,
/// reading only the file headers.
///
/// For RAW files this is the size of the developed image (the sensor's
/// recommended crop), not of the embedded preview.
pub fn dimensions(path: &Path, kind: SourceKind) -> Result<(u32, u32)> {
    let (w, h, orientation) = match kind {
        SourceKind::Image => {
            let mut decoder = ImageReader::open(path)?
                .with_guessed_format()?
                .into_decoder()
                .context("cannot read image file")?;
            let (w, h) = decoder.dimensions();
            let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
            (w, h, orientation)
        }
        SourceKind::Raw => {
            let src = RawSource::new(path).context("cannot open file")?;
            let decoder =
                rawler::get_decoder(&src).map_err(|e| anyhow!("unsupported RAW file: {e}"))?;
            let params = RawDecodeParams::default();
            // `dummy = true` parses the metadata without decoding sensor data.
            let raw = decoder
                .raw_image(&src, &params, true)
                .map_err(|e| anyhow!("failed to read RAW header: {e}"))?;
            let (w, h) = raw
                .crop_area
                .or(raw.active_area)
                .map(|r| (r.d.w, r.d.h))
                .unwrap_or((raw.width, raw.height));
            let orientation = raw_orientation(decoder.as_ref(), &src, &params);
            (w as u32, h as u32, orientation)
        }
    };
    Ok(if swaps_axes(orientation) {
        (h, w)
    } else {
        (w, h)
    })
}

fn swaps_axes(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

fn raw_orientation(
    decoder: &dyn Decoder,
    src: &RawSource,
    params: &RawDecodeParams,
) -> Orientation {
    decoder
        .raw_metadata(src, params)
        .ok()
        .and_then(|md| md.exif.orientation)
        .and_then(|o| Orientation::from_exif(o as u8))
        .unwrap_or(Orientation::NoTransforms)
}

/// Decoded size above which images are decoded one at a time.
const LARGE_IMAGE_BYTES: u64 = 512 * 1024 * 1024;

/// Held while a large image is decoded. Photos load in parallel, and several
/// film scans of a few GB each decoded at once could exhaust memory.
static LARGE_IMAGE_DECODE: Mutex<()> = Mutex::new(());

fn load_image(path: &Path, min_size: u32) -> Result<DynamicImage> {
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    // The default 512 MiB limit rejects large film scans; memory use is
    // bounded by decoding them one at a time instead.
    reader.no_limits();
    let mut decoder = reader.into_decoder().context("cannot read image file")?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let large = decoder.total_bytes() > LARGE_IMAGE_BYTES;
    let guard = large.then(|| LARGE_IMAGE_DECODE.lock().unwrap_or_else(|e| e.into_inner()));
    let mut img = DynamicImage::from_decoder(decoder).context("failed to decode image")?;
    if large {
        // Shrink while still holding the lock, so only one full-size scan is
        // in memory at a time. Twice the thumbnail size, as the thumbnail
        // pre-shrink would do anyway – the result looks the same.
        let side = min_size.saturating_mul(2).max(1);
        img = img.thumbnail(side, side);
    }
    drop(guard);
    img.apply_orientation(orientation);
    Ok(img)
}

fn load_raw(path: &Path, min_size: u32) -> Result<DynamicImage> {
    let src = RawSource::new(path).context("cannot open file")?;
    let decoder = match rawler::get_decoder(&src) {
        Ok(decoder) => decoder,
        Err(e) if has_ifd0_preview(path) => {
            return load_ifd0_preview(&src).map_err(|_| anyhow!("unsupported RAW file: {e}"));
        }
        Err(e) => bail!("unsupported RAW file: {e}"),
    };
    let params = RawDecodeParams::default();
    let orientation = raw_orientation(decoder.as_ref(), &src, &params);

    let big_enough = |img: &DynamicImage| img.width().max(img.height()) >= min_size;

    // Fastest: the embedded JPEG preview. If it is missing or too small,
    // develop the raw sensor data.
    let preview = decoder
        .preview_image(&src, &params)
        .ok()
        .flatten()
        .filter(big_enough)
        .or_else(|| {
            decoder
                .thumbnail_image(&src, &params)
                .ok()
                .flatten()
                .filter(big_enough)
        });

    let mut img = match preview {
        Some(img) => img,
        None => {
            let raw = decoder
                .raw_image(&src, &params, false)
                .map_err(|e| anyhow!("failed to decode RAW: {e}"))?;
            RawDevelop::default()
                .develop_intermediate(&raw)
                .map_err(|e| anyhow!("failed to develop RAW: {e}"))?
                .to_dynamic_image()
                .ok_or_else(|| anyhow!("failed to convert developed image"))?
        }
    };
    img.apply_orientation(orientation);
    Ok(img)
}

/// Whether the format keeps a full-size preview in its first TIFF IFD that can be
/// read when rawler does not know the camera.
///
/// rawler only accepts Hasselblad models listed in its camera database, matched by
/// the exact model name. CFV backs write the body or shutter mode into that name
/// (e.g. "Hasselblad CFV-50c/500", "CFV 100C/Electronic Shutter"), so their files
/// are rejected even though the preview is perfectly readable.
fn has_ifd0_preview(path: &Path) -> bool {
    matches!(lowercase_extension(path).as_deref(), Some("3fr" | "fff"))
}

/// Reads the preview stored in the first IFD: uncompressed 8-bit RGB (older
/// Hasselblad cameras) or JPEG (X2D, CFV 100C and newer).
fn load_ifd0_preview(src: &RawSource) -> Result<DynamicImage> {
    let tiff = GenericTiffReader::new(&mut src.reader(), 0, 0, None, &[])
        .map_err(|e| anyhow!("cannot read TIFF structure: {e}"))?;
    let ifd = tiff.root_ifd();
    let tag = |t: TiffCommonTag| ifd.get_entry(t).map(|e| e.force_usize(0));

    let (strips, _) = ifd
        .strip_data(src)
        .map_err(|e| anyhow!("cannot read preview data: {e}"))?;
    let buf = strips.concat();
    let mut img = match tag(TiffCommonTag::Compression) {
        Some(1)
            if tag(TiffCommonTag::BitsPerSample) == Some(8)
                && tag(TiffCommonTag::SamplesPerPixel) == Some(3) =>
        {
            let width = tag(TiffCommonTag::ImageWidth).context("missing preview width")?;
            let height = tag(TiffCommonTag::ImageLength).context("missing preview height")?;
            let rgb = RgbImage::from_raw(width as u32, height as u32, buf)
                .context("preview data is truncated")?;
            DynamicImage::ImageRgb8(rgb)
        }
        Some(6 | 7) => image::load_from_memory_with_format(&buf, ImageFormat::Jpeg)
            .context("failed to decode JPEG preview")?,
        _ => bail!("unsupported preview format"),
    };
    let orientation = tag(TiffCommonTag::Orientation)
        .and_then(|o| Orientation::from_exif(o as u8))
        .unwrap_or(Orientation::NoTransforms);
    img.apply_orientation(orientation);
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_filters_expand_to_extensions() {
        assert_eq!(format_filter("RAW"), Some(RAW_EXTENSIONS));
        assert_eq!(format_filter("cr3"), Some(&["cr3"][..]));
        assert_eq!(format_filter(".NEF"), Some(&["nef"][..]));
        assert_eq!(format_filter("jpeg"), Some(&["jpg", "jpeg"][..]));
        assert_eq!(format_filter("TIF"), Some(&["tif", "tiff"][..]));
        assert_eq!(format_filter("png"), None);
        assert_eq!(format_filter(""), None);
        // Every value listed in the help text is accepted.
        for name in FORMAT_FILTERS.split(", ") {
            assert!(format_filter(name).is_some(), "{name}");
        }
    }

    #[test]
    fn detects_kind_by_extension() {
        let kind = |name: &str| source_kind(Path::new(name));
        assert_eq!(kind("IMG_0001.CR2"), Some(SourceKind::Raw));
        assert_eq!(kind("IMG_0001.cr3"), Some(SourceKind::Raw));
        assert_eq!(kind("DSC_0001.NEF"), Some(SourceKind::Raw));
        assert_eq!(kind("DSCN0001.nrw"), Some(SourceKind::Raw));
        assert_eq!(kind("DSCF0001.RAF"), Some(SourceKind::Raw));
        assert_eq!(kind("B0000001.3FR"), Some(SourceKind::Raw));
        assert_eq!(kind("B0000001.fff"), Some(SourceKind::Raw));
        assert_eq!(kind("pano.dng"), Some(SourceKind::Raw));
        assert_eq!(kind("scan.TIF"), Some(SourceKind::Image));
        assert_eq!(kind("photo.jpg"), Some(SourceKind::Image));
        assert_eq!(kind("photo.JPEG"), Some(SourceKind::Image));
        assert_eq!(kind("notes.txt"), None);
        assert_eq!(kind("no_extension"), None);
    }

    #[test]
    fn jpeg_exif_orientation_is_applied() {
        use image::codecs::jpeg::JpegEncoder;
        use image::{ImageEncoder, RgbImage};

        // Minimal little-endian TIFF/EXIF block with a single tag:
        // Orientation (0x0112) = 6, i.e. rotate 90° clockwise.
        let exif = vec![
            b'I', b'I', 42, 0, 8, 0, 0, 0, // header, IFD0 at offset 8
            1, 0, // one entry
            0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, // Orientation, SHORT, 1, 6
            0, 0, 0, 0, // no next IFD
        ];
        let path =
            std::env::temp_dir().join(format!("contact-sheet-test-{}.jpg", std::process::id()));
        let mut buf = Vec::new();
        let mut enc = JpegEncoder::new(&mut buf);
        enc.set_exif_metadata(exif).unwrap();
        enc.write_image(
            RgbImage::new(40, 20).as_raw(),
            40,
            20,
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
        std::fs::write(&path, buf).unwrap();

        let img = load(&path, SourceKind::Image, 0);
        std::fs::remove_file(&path).unwrap();
        let img = img.unwrap();
        assert_eq!((img.width(), img.height()), (20, 40));
    }

    #[test]
    fn ifd0_rgb_preview_is_read_and_oriented() {
        // Little-endian TIFF whose first IFD holds a 2×1 uncompressed RGB image
        // (a red and a blue pixel) with Orientation = 6 (rotate 90° clockwise).
        let entry = |tag: u16, typ: u16, value: u32| {
            let mut e = Vec::new();
            e.extend_from_slice(&tag.to_le_bytes());
            e.extend_from_slice(&typ.to_le_bytes());
            e.extend_from_slice(&1u32.to_le_bytes());
            e.extend_from_slice(&value.to_le_bytes());
            e
        };
        const SHORT: u16 = 3;
        const LONG: u16 = 4;
        let entries = [
            entry(256, SHORT, 2),  // ImageWidth
            entry(257, SHORT, 1),  // ImageLength
            entry(258, SHORT, 8),  // BitsPerSample
            entry(259, SHORT, 1),  // Compression: none
            entry(262, SHORT, 2),  // PhotometricInterpretation: RGB
            entry(273, LONG, 122), // StripOffsets: right after the IFD
            entry(274, SHORT, 6),  // Orientation
            entry(277, SHORT, 3),  // SamplesPerPixel
            entry(279, LONG, 6),   // StripByteCounts
        ];
        let mut tiff = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
        tiff.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        entries.iter().for_each(|e| tiff.extend_from_slice(e));
        tiff.extend_from_slice(&[0, 0, 0, 0]); // no next IFD
        assert_eq!(tiff.len(), 122);
        tiff.extend_from_slice(&[255, 0, 0, 0, 0, 255]);

        let img = load_ifd0_preview(&RawSource::new_from_slice(&tiff))
            .unwrap()
            .to_rgb8();
        assert_eq!(img.dimensions(), (1, 2));
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0]);
        assert_eq!(img.get_pixel(0, 1).0, [0, 0, 255]);
    }
}
