//! Loading source photos (Canon, Nikon and Fujifilm RAW, DNG, TIFF, JPEG) into a `DynamicImage`.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader};
use rawler::decoders::{Decoder, RawDecodeParams};
use rawler::imgop::develop::RawDevelop;
use rawler::rawsource::RawSource;

/// Kind of supported input file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// Canon (CR2, CR3, CRW), Nikon (NEF, NRW) and Fujifilm (RAF) RAW, and DNG –
    /// decoded by `rawler`.
    Raw,
    /// TIFF (including uncompressed, 8/16 bit) and JPEG – decoded by `image`.
    Image,
}

/// Detects the file kind from its extension (case-insensitive).
pub fn source_kind(path: &Path) -> Option<SourceKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "cr2" | "cr3" | "crw" | "nef" | "nrw" | "raf" | "dng" => Some(SourceKind::Raw),
        "tif" | "tiff" | "jpg" | "jpeg" => Some(SourceKind::Image),
        _ => None,
    }
}

/// Human-readable format name of a supported file, based on its extension.
pub fn format_name(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "cr2" => "Canon RAW (CR2)",
        "cr3" => "Canon RAW (CR3)",
        "crw" => "Canon RAW (CRW)",
        "nef" => "Nikon RAW (NEF)",
        "nrw" => "Nikon RAW (NRW)",
        "raf" => "Fujifilm RAW (RAF)",
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
        SourceKind::Image => load_image(path),
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

fn load_image(path: &Path) -> Result<DynamicImage> {
    let mut decoder = ImageReader::open(path)?
        .with_guessed_format()?
        .into_decoder()
        .context("cannot read image file")?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).context("failed to decode image")?;
    img.apply_orientation(orientation);
    Ok(img)
}

fn load_raw(path: &Path, min_size: u32) -> Result<DynamicImage> {
    let src = RawSource::new(path).context("cannot open file")?;
    let decoder = rawler::get_decoder(&src).map_err(|e| anyhow!("unsupported RAW file: {e}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_kind_by_extension() {
        let kind = |name: &str| source_kind(Path::new(name));
        assert_eq!(kind("IMG_0001.CR2"), Some(SourceKind::Raw));
        assert_eq!(kind("IMG_0001.cr3"), Some(SourceKind::Raw));
        assert_eq!(kind("DSC_0001.NEF"), Some(SourceKind::Raw));
        assert_eq!(kind("DSCN0001.nrw"), Some(SourceKind::Raw));
        assert_eq!(kind("DSCF0001.RAF"), Some(SourceKind::Raw));
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
}
