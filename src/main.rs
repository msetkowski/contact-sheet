mod loader;
mod sheet;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use rayon::prelude::*;

use loader::source_kind;
use sheet::{Layout, Paginator, Palette, Renderer, Thumb, make_thumbnail};

const OUTPUT_DIR: &str = "contact-sheet";
/// Number of rows loaded in parallel in one batch.
const BATCH_ROWS: usize = 8;

/// Creates A4 contact sheets from the photos in a directory.
///
/// Supported formats: Canon RAW (CR2, CR3, CRW), Nikon RAW (NEF, NRW),
/// Fujifilm RAW (RAF), DNG, TIFF, JPEG.
/// Output is written to the `contact-sheet` subdirectory as contact01.jpg, contact02.jpg, …
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Directory containing the photos
    input: PathBuf,

    /// Number of thumbnail columns
    #[arg(short, long, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..=20))]
    columns: u32,

    /// Sheet resolution (DPI); 300 DPI gives 2480×3508 px
    #[arg(short, long, default_value_t = 300, value_parser = clap::value_parser!(u16).range(72..=1200))]
    dpi: u16,

    /// JPEG quality (1–100)
    #[arg(short, long, default_value_t = 90, value_parser = clap::value_parser!(u8).range(1..=100))]
    quality: u8,

    /// Use landscape page orientation
    #[arg(short, long)]
    landscape: bool,

    /// Sheet background colour
    #[arg(short, long, value_enum, default_value_t = Background::White)]
    background: Background,

    /// Print the name, file size, pixel dimensions and format of every file found
    #[arg(short, long)]
    verbose: bool,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
enum Background {
    /// White background, dark labels (for printing)
    White,
    /// Black background, white labels (for on-screen viewing)
    Black,
}

impl Background {
    fn palette(self) -> Palette {
        match self {
            Background::White => Palette::WHITE,
            Background::Black => Palette::BLACK,
        }
    }
}

fn main() {
    if let Err(e) = run(Args::parse()) {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    if !args.input.is_dir() {
        bail!("{} is not a directory", args.input.display());
    }

    let files = collect_files(&args.input)?;
    if files.is_empty() {
        bail!(
            "no supported files (CR2, CR3, CRW, NEF, NRW, RAF, DNG, TIFF, JPEG) found in {}",
            args.input.display()
        );
    }
    println!("Found {} file(s)", files.len());
    if args.verbose {
        print_file_list(&files);
    }

    let layout = Layout::a4(args.dpi, args.columns, args.landscape);
    if !layout.fits_tallest_row() {
        bail!("with these settings a row of thumbnails does not fit on the page");
    }
    let renderer = Renderer::new(layout.clone(), args.background.palette())?;
    let columns = layout.columns as usize;

    let out_dir = args.input.join(OUTPUT_DIR);
    fs::create_dir_all(&out_dir).with_context(|| format!("cannot create {}", out_dir.display()))?;
    remove_old_sheets(&out_dir)?;

    let mut paginator = Paginator::new(layout.clone());
    let mut pending: Vec<Thumb> = Vec::new();
    let mut page_no = 0;
    let mut failed = 0;
    let mut write_page = |rows: Vec<Vec<Thumb>>| -> Result<()> {
        page_no += 1;
        let path = out_dir.join(format!("contact{page_no:02}.jpg"));
        renderer.save_jpeg(&renderer.render_page(&rows), &path, args.quality)?;
        let count: usize = rows.iter().map(Vec::len).sum();
        println!("Saved {} ({count} photo(s))", path.display());
        Ok(())
    };

    // Load in batches (in parallel within a batch) so we never keep
    // thumbnails of the whole directory in memory.
    for batch in files.chunks(columns * BATCH_ROWS) {
        let thumbs: Vec<Thumb> = batch
            .par_iter()
            .filter_map(|(path, kind)| {
                let name = file_name(path);
                match loader::load(path, *kind, layout.cell) {
                    Ok(img) => Some(Thumb {
                        image: make_thumbnail(&img, layout.cell),
                        label: name,
                    }),
                    Err(e) => {
                        eprintln!("Skipped {name}: {e:#}");
                        None
                    }
                }
            })
            .collect();
        failed += batch.len() - thumbs.len();
        pending.extend(thumbs);

        while pending.len() >= columns {
            let row: Vec<Thumb> = pending.drain(..columns).collect();
            if let Some(page) = paginator.push_row(row) {
                write_page(page)?;
            }
        }
    }
    if !pending.is_empty()
        && let Some(page) = paginator.push_row(pending)
    {
        write_page(page)?;
    }
    if let Some(page) = paginator.finish() {
        write_page(page)?;
    }

    if page_no == 0 {
        bail!("could not load any photo");
    }
    if failed > 0 {
        eprintln!("Failed to load {failed} file(s)");
    }
    Ok(())
}

/// Returns supported files from the directory (non-recursive), sorted by name.
fn collect_files(dir: &Path) -> Result<Vec<(PathBuf, loader::SourceKind)>> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .with_context(|| format!("cannot read {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| !is_hidden(p))
        .filter_map(|p| source_kind(&p).map(|k| (p, k)))
        .collect();
    files.sort_by_cached_key(|(p, _)| {
        p.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
    });
    Ok(files)
}

/// Prints one aligned line per file: name, file size, pixel dimensions and format.
fn print_file_list(files: &[(PathBuf, loader::SourceKind)]) {
    let names: Vec<String> = files.iter().map(|(p, _)| file_name(p)).collect();
    let width = names.iter().map(|n| n.chars().count()).max().unwrap_or(0);
    // Reading headers is cheap, but RAW parsing still adds up across a directory.
    let dims: Vec<String> = files
        .par_iter()
        .map(|(path, kind)| match loader::dimensions(path, *kind) {
            Ok((w, h)) => format!("{w}×{h}"),
            Err(_) => "?".to_string(),
        })
        .collect();
    for (((path, _), name), dims) in files.iter().zip(&names).zip(&dims) {
        let size = fs::metadata(path)
            .map(|m| format_size(m.len()))
            .unwrap_or_else(|_| "?".to_string());
        println!(
            "  {name:<width$}  {size:>9}  {dims:>11}  {}",
            loader::format_name(path)
        );
    }
}

/// Formats a byte count with decimal units (as shown by Finder), e.g. `24.7 MB`.
fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Skips hidden files, e.g. `._IMG_0001.CR2` created by macOS on exFAT drives.
fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}

/// Removes contact sheets from a previous run so no stale pages are left behind.
fn remove_old_sheets(out_dir: &Path) -> Result<()> {
    for entry in fs::read_dir(out_dir)?.filter_map(|e| e.ok()) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let is_sheet = name
            .strip_prefix("contact")
            .and_then(|r| r.strip_suffix(".jpg"))
            .is_some_and(|n| n.len() >= 2 && n.bytes().all(|b| b.is_ascii_digit()));
        if is_sheet {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_use_decimal_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1_500), "1.5 KB");
        assert_eq!(format_size(24_700_000), "24.7 MB");
        assert_eq!(format_size(3_200_000_000), "3.2 GB");
    }
}
