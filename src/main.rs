mod loader;
mod pdf;
mod sheet;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use rayon::prelude::*;

use loader::{SourceKind, lowercase_extension, source_kind};
use pdf::{DEFAULT_MAX_PDF_BYTES, OutputIntent, PdfSeries};
use sheet::{Layout, Paginator, Palette, Paper, Renderer, Thumb, make_thumbnail};

const OUTPUT_DIR: &str = "contact-sheet";
/// PDF files above this size get a warning, whatever --max-pdf-size allows.
const LARGE_PDF_BYTES: u64 = 1_000_000_000;
/// Number of rows loaded in parallel in one batch.
const BATCH_ROWS: usize = 8;

/// Creates contact sheets (A4, A3 or Letter) from the photos in a directory.
///
/// Supported formats: Canon RAW (CR2, CR3, CRW), Nikon RAW (NEF, NRW),
/// Fujifilm RAW (RAF), Hasselblad RAW (3FR, FFF), DNG, TIFF, JPEG.
/// Output is written to the `contact-sheet` subdirectory as contact01.jpg, contact02.jpg, …
/// or, with `--format pdf`, as a single PDF/X-4 file contact.pdf.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Directory containing the photos
    input: PathBuf,

    /// Also scan subdirectories: all levels, or at most DEPTH levels down
    /// (e.g. `--recursive 2`). Hidden directories are skipped.
    #[arg(
        short,
        long,
        value_name = "DEPTH",
        num_args = 0..=1,
        require_equals = true,
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    recursive: Option<Option<u32>>,

    /// Include only these formats, e.g. `--only raw` or `--only cr3,jpg` (for
    /// cameras saving RAW + JPEG). `raw` means every RAW format and DNG.
    #[arg(
        long,
        value_name = "FORMAT",
        value_delimiter = ',',
        value_parser = parse_format_filter
    )]
    only: Vec<&'static [&'static str]>,

    /// Paper size
    #[arg(short, long, value_enum, ignore_case = true, default_value_t = PaperSize::A4)]
    paper: PaperSize,

    /// Number of thumbnail columns
    #[arg(short, long, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..=20))]
    columns: u32,

    /// Sheet resolution (DPI, 72–600); 300 DPI gives 2480×3508 px on A4
    #[arg(short, long, default_value_t = 300, value_parser = clap::value_parser!(u16).range(72..=600))]
    dpi: u16,

    /// Output format
    #[arg(short, long, value_enum, default_value_t = Format::Jpg)]
    format: Format,

    /// ICC output profile of the printing condition for PDF output, e.g. one from
    /// your print shop (default: built-in FOGRA39)
    #[arg(long, value_name = "FILE")]
    output_profile: Option<PathBuf>,

    /// Largest size of one PDF file in MB; larger output is split into
    /// contact-01.pdf, contact-02.pdf, … (default: 500)
    #[arg(long, value_name = "MB", value_parser = clap::value_parser!(u64).range(1..=1_000_000))]
    max_pdf_size: Option<u64>,

    /// Largest number of pages in one PDF file; more pages are split into
    /// several files
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    max_pages: Option<u32>,

    /// JPEG quality (1–100), also used for the page images in a PDF
    #[arg(short, long, default_value_t = 90, value_parser = clap::value_parser!(u8).range(1..=100))]
    quality: u8,

    /// Use landscape page orientation
    #[arg(short, long)]
    landscape: bool,

    /// Sheet background colour
    #[arg(short, long, value_enum, default_value_t = Background::White)]
    background: Background,

    /// Show thumbnails only, without file names below them
    #[arg(long)]
    no_labels: bool,

    /// Print the name, file size, pixel dimensions and format of every file found
    #[arg(short, long)]
    verbose: bool,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    /// One JPEG file per page (contact01.jpg, contact02.jpg, …)
    Jpg,
    /// All pages in a single PDF/X-4 file (contact.pdf) with a FOGRA39 output intent,
    /// or the one given by --output-profile
    Pdf,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum PaperSize {
    /// 210 × 297 mm
    A4,
    /// 297 × 420 mm
    A3,
    /// US Letter, 8.5 × 11 in
    Letter,
}

impl PaperSize {
    fn paper(self) -> Paper {
        match self {
            PaperSize::A4 => Paper::A4,
            PaperSize::A3 => Paper::A3,
            PaperSize::Letter => Paper::LETTER,
        }
    }
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

/// A photo to put on the sheet.
struct SourceFile {
    path: PathBuf,
    kind: SourceKind,
    /// Path relative to the input directory, with `/` separators – the label.
    name: String,
}

fn main() {
    if let Err(e) = run(Args::parse_from(normalize_args(std::env::args_os()))) {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    if !args.input.is_dir() {
        bail!("{} is not a directory", args.input.display());
    }
    if args.format == Format::Jpg {
        for (given, option) in [
            (args.output_profile.is_some(), "--output-profile"),
            (args.max_pdf_size.is_some(), "--max-pdf-size"),
            (args.max_pages.is_some(), "--max-pages"),
        ] {
            if given {
                bail!("{option} only applies to --format pdf");
            }
        }
    }
    // Check the profile before the slow part, so a bad file fails right away.
    let intent = match &args.output_profile {
        Some(path) => OutputIntent::from_icc_file(path)?,
        None => OutputIntent::fogra39(),
    };

    // None: no depth limit; Some(0): the input directory only.
    let max_depth = match args.recursive {
        None => Some(0),
        Some(depth) => depth,
    };
    let only = args.only.concat();
    let (files, excluded) = collect_files(&args.input, max_depth, &only)?;
    if files.is_empty() && excluded > 0 {
        bail!(
            "no {} files found in {} ({excluded} file(s) in other formats left out by --only)",
            only.join("/").to_uppercase(),
            args.input.display()
        );
    }
    if files.is_empty() {
        bail!(
            "no supported files (CR2, CR3, CRW, NEF, NRW, RAF, 3FR, FFF, DNG, TIFF, JPEG) found in {}",
            args.input.display()
        );
    }
    if excluded > 0 {
        println!(
            "Found {} file(s); {excluded} file(s) in other formats left out by --only",
            files.len()
        );
    } else {
        println!("Found {} file(s)", files.len());
    }
    if args.verbose {
        print_file_list(&files);
    }

    let mut layout = Layout::new(args.paper.paper(), args.dpi, args.columns, args.landscape);
    if args.no_labels {
        layout = layout.without_labels();
    }
    if !layout.fits_tallest_row() {
        bail!("with these settings a row of thumbnails does not fit on the page");
    }
    let renderer = Renderer::new(layout.clone(), args.background.palette())?;
    let columns = layout.columns as usize;

    let out_dir = args.input.join(OUTPUT_DIR);
    fs::create_dir_all(&out_dir).with_context(|| format!("cannot create {}", out_dir.display()))?;
    let max_pdf_bytes = args
        .max_pdf_size
        .map_or(DEFAULT_MAX_PDF_BYTES, |mb| mb * 1_000_000);
    let mut pdf = match args.format {
        Format::Jpg => {
            remove_old_sheets(&out_dir)?;
            None
        }
        Format::Pdf => {
            println!("PDF/X-4 output intent: {}", intent.name());
            let max_pages = args.max_pages.map(|n| n as usize);
            let title = pdf_title(&args.input);
            Some(PdfSeries::new(
                &out_dir,
                &title,
                intent,
                max_pages,
                max_pdf_bytes,
            )?)
        }
    };

    let mut paginator = Paginator::new(layout.clone());
    let mut pending: Vec<Thumb> = Vec::new();
    let mut page_no = 0;
    let mut photos = 0;
    let mut failed = 0;
    let total_files = files.len();
    let mut write_page = |rows: Vec<Vec<Thumb>>| -> Result<()> {
        page_no += 1;
        let page = renderer.render_page(&rows);
        let count: usize = rows.iter().map(Vec::len).sum();
        photos += count;
        match &mut pdf {
            Some(pdf) => {
                let jpeg = renderer.encode_jpeg(&page, args.quality)?;
                if page_no == 1 {
                    warn_about_pdf_size(
                        jpeg.len() as u64,
                        total_files.div_ceil(count),
                        max_pdf_bytes,
                    );
                }
                pdf.add_page(&jpeg, page.width(), page.height(), layout.page_size_pt)?;
                println!("Rendered page {page_no} ({count} photo(s))");
            }
            None => {
                let path = out_dir.join(format!("contact{page_no:02}.jpg"));
                renderer.save_jpeg(&page, &path, args.quality)?;
                println!("Saved {} ({count} photo(s))", path.display());
            }
        }
        Ok(())
    };

    // Load in batches (in parallel within a batch) so we never keep
    // thumbnails of the whole directory in memory.
    for batch in files.chunks(columns * BATCH_ROWS) {
        let thumbs: Vec<Thumb> = batch
            .par_iter()
            .filter_map(
                |file| match loader::load(&file.path, file.kind, layout.cell) {
                    Ok(img) => Some(Thumb {
                        image: make_thumbnail(&img, layout.cell),
                        label: file.name.clone(),
                    }),
                    Err(e) => {
                        eprintln!("Skipped {}: {e:#}", file.name);
                        None
                    }
                },
            )
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
    if let Some(pdf) = pdf {
        let saved = pdf.finish()?;
        for (path, pages) in &saved {
            println!("Saved {} ({pages} page(s))", path.display());
        }
        println!(
            "{photos} photo(s) on {page_no} page(s) in {} PDF file(s)",
            saved.len()
        );
    }
    if failed > 0 {
        eprintln!("Failed to load {failed} file(s)");
    }
    Ok(())
}

/// Tells up front how large the PDF output is going to be, extrapolated from
/// the size of the first page and the number of photos it holds.
fn warn_about_pdf_size(page_bytes: u64, estimated_pages: usize, max_file_bytes: u64) {
    let total = page_bytes * estimated_pages as u64;
    if total > max_file_bytes {
        let files = total.div_ceil(max_file_bytes);
        println!(
            "Note: the PDF output will be about {} ({estimated_pages} pages); it is split into \
             about {files} files of at most {} each (see --max-pdf-size)",
            format_size(total),
            format_size(max_file_bytes)
        );
    }
    if total.min(max_file_bytes) > LARGE_PDF_BYTES {
        eprintln!(
            "Warning: PDF files over {} open slowly or not at all in many viewers and \
             print shop systems; consider a lower --max-pdf-size",
            format_size(LARGE_PDF_BYTES)
        );
    }
}

/// Document title of the PDF, e.g. "Contact sheet – 2026-10-17".
fn pdf_title(input: &Path) -> String {
    let dir = input
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| input.display().to_string());
    format!("Contact sheet – {dir}")
}

/// Lets `--recursive` take its depth as a separate argument (`--recursive 2`).
///
/// clap would otherwise read the next argument as the depth, so
/// `--recursive ~/Photos` would fail. The option therefore only accepts
/// `--recursive=2`, and a following number is joined to it here.
fn normalize_args(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut out = Vec::new();
    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        if arg == "--" {
            out.push(arg);
            out.extend(args);
            break;
        }
        let depth = args
            .peek()
            .and_then(|next| next.to_str())
            .filter(|next| !next.is_empty() && next.bytes().all(|b| b.is_ascii_digit()))
            .map(str::to_owned);
        match depth {
            Some(depth) if arg == "--recursive" || arg == "-r" => {
                args.next();
                out.push(format!("--recursive={depth}").into());
            }
            _ => out.push(arg),
        }
    }
    out
}

fn parse_format_filter(value: &str) -> Result<&'static [&'static str], String> {
    loader::format_filter(value)
        .ok_or_else(|| format!("unknown format; use one of: {}", loader::FORMAT_FILTERS))
}

/// Returns supported files sorted by their path relative to `root`, and the
/// number of supported files left out because their extension is not in `only`
/// (an empty `only` keeps every supported file).
///
/// `max_depth` limits how many directory levels below `root` are scanned
/// (`Some(0)`: `root` only, `None`: no limit). Hidden directories and output
/// directories of earlier runs (`contact-sheet`) are skipped, as are symbolic
/// links to directories, which could form loops.
fn collect_files(
    root: &Path,
    max_depth: Option<u32>,
    only: &[&str],
) -> Result<(Vec<SourceFile>, usize)> {
    let mut files = Vec::new();
    let mut excluded = 0;
    let mut dirs = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if dir == root => {
                return Err(e).with_context(|| format!("cannot read {}", dir.display()));
            }
            Err(e) => {
                eprintln!("Skipped directory {}: {e}", dir.display());
                continue;
            }
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if is_hidden(&path) {
                continue;
            }
            // `file_type` does not follow symbolic links.
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                if max_depth.is_none_or(|max| depth < max) && entry.file_name() != OUTPUT_DIR {
                    dirs.push((path, depth + 1));
                }
            } else if path.is_file()
                && let Some(kind) = source_kind(&path)
            {
                let wanted = only.is_empty()
                    || lowercase_extension(&path).is_some_and(|ext| only.contains(&ext.as_str()));
                if !wanted {
                    excluded += 1;
                    continue;
                }
                let name = relative_name(&path, root);
                files.push(SourceFile { path, kind, name });
            }
        }
    }
    files.sort_by_cached_key(|f| f.name.to_lowercase());
    Ok((files, excluded))
}

/// `path` relative to `root`, joined with `/` on every platform.
fn relative_name(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Prints one aligned line per file: name, file size, pixel dimensions and format.
fn print_file_list(files: &[SourceFile]) {
    let width = files
        .iter()
        .map(|f| f.name.chars().count())
        .max()
        .unwrap_or(0);
    // Reading headers is cheap, but RAW parsing still adds up across a directory.
    let dims: Vec<String> = files
        .par_iter()
        .map(|f| match loader::dimensions(&f.path, f.kind) {
            Ok((w, h)) => format!("{w}×{h}"),
            Err(_) => "?".to_string(),
        })
        .collect();
    for (file, dims) in files.iter().zip(&dims) {
        let size = fs::metadata(&file.path)
            .map(|m| format_size(m.len()))
            .unwrap_or_else(|_| "?".to_string());
        println!(
            "  {:<width$}  {size:>9}  {dims:>11}  {}",
            file.name,
            loader::format_name(&file.path)
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
    use loader::format_filter;

    fn parse(args: &[&str]) -> Result<Args, clap::Error> {
        let args = std::iter::once("contact-sheet").chain(args.iter().copied());
        Args::try_parse_from(normalize_args(args.map(OsString::from)))
    }

    #[test]
    fn recursive_takes_an_optional_separate_depth() {
        let a = parse(&["photos"]).unwrap();
        assert_eq!(a.recursive, None);

        let a = parse(&["--recursive", "photos"]).unwrap();
        assert_eq!(
            (a.recursive, a.input),
            (Some(None), PathBuf::from("photos"))
        );

        let a = parse(&["photos", "--recursive"]).unwrap();
        assert_eq!(a.recursive, Some(None));

        let a = parse(&["--recursive", "2", "photos"]).unwrap();
        assert_eq!(
            (a.recursive, a.input),
            (Some(Some(2)), PathBuf::from("photos"))
        );

        let a = parse(&["-r", "3", "photos"]).unwrap();
        assert_eq!(a.recursive, Some(Some(3)));

        let a = parse(&["--recursive=1", "photos"]).unwrap();
        assert_eq!(a.recursive, Some(Some(1)));

        assert!(parse(&["--recursive", "0", "photos"]).is_err());
        // After `--` every argument is taken literally, even a number.
        let a = parse(&["-r", "--", "2"]).unwrap();
        assert_eq!((a.recursive, a.input), (Some(None), PathBuf::from("2")));
    }

    #[test]
    fn paper_and_dpi_options() {
        assert_eq!(parse(&["photos"]).unwrap().paper, PaperSize::A4);
        assert_eq!(
            parse(&["--paper", "A3", "photos"]).unwrap().paper,
            PaperSize::A3
        );
        assert_eq!(
            parse(&["-p", "letter", "photos"]).unwrap().paper,
            PaperSize::Letter
        );
        assert!(parse(&["--paper", "a5", "photos"]).is_err());
        assert_eq!(parse(&["--dpi", "600", "photos"]).unwrap().dpi, 600);
        assert!(parse(&["--dpi", "601", "photos"]).is_err());
    }

    #[test]
    fn only_accepts_lists_and_repeats() {
        assert!(parse(&["photos"]).unwrap().only.is_empty());
        let only = parse(&["--only", "CR3,jpg", "photos"])
            .unwrap()
            .only
            .concat();
        assert_eq!(only, ["cr3", "jpg", "jpeg"]);
        let only = parse(&["--only", "raw", "--only", "tif", "photos"])
            .unwrap()
            .only
            .concat();
        assert!(only.contains(&"nef") && only.contains(&"dng") && only.contains(&"tiff"));
        let err = parse(&["--only", "png", "photos"]).unwrap_err().to_string();
        assert!(err.contains("use one of"), "{err}");
    }

    #[test]
    fn only_keeps_the_chosen_formats() {
        let root = std::env::temp_dir().join(format!("contact-sheet-only-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for file in [
            "IMG_1.CR3",
            "IMG_1.JPG",
            "IMG_2.cr3",
            "IMG_2.jpeg",
            "scan.tif",
            "a.txt",
        ] {
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join(file), b"").unwrap();
        }
        let names = |only: &[&str]| -> (Vec<String>, usize) {
            let (files, excluded) = collect_files(&root, Some(0), only).unwrap();
            (files.into_iter().map(|f| f.name).collect(), excluded)
        };

        let all = names(&[]);
        let raw = names(format_filter("raw").unwrap());
        let jpg = names(format_filter("jpg").unwrap());
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(all.0.len(), 5);
        assert_eq!(all.1, 0);
        assert_eq!(raw, (vec!["IMG_1.CR3".into(), "IMG_2.cr3".into()], 3));
        assert_eq!(jpg, (vec!["IMG_1.JPG".into(), "IMG_2.jpeg".into()], 3));
    }

    #[test]
    fn collects_files_down_to_the_given_depth() {
        let root = std::env::temp_dir().join(format!("contact-sheet-walk-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for file in [
            "b.jpg",
            "notes.txt",
            "A/c.JPG",
            "A/deeper/d.cr2",
            "A/deeper/deepest/e.dng",
            ".hidden/f.jpg",
            "A/._g.jpg",
            "contact-sheet/contact01.jpg",
            "A/contact-sheet/contact01.jpg",
        ] {
            let path = root.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"").unwrap();
        }
        let names = |depth| -> Vec<String> {
            let (files, _) = collect_files(&root, depth, &[]).unwrap();
            files.into_iter().map(|f| f.name).collect()
        };

        let top = names(Some(0));
        let two = names(Some(2));
        let all = names(None);
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(top, ["b.jpg"]);
        assert_eq!(two, ["A/c.JPG", "A/deeper/d.cr2", "b.jpg"]);
        assert_eq!(
            all,
            [
                "A/c.JPG",
                "A/deeper/d.cr2",
                "A/deeper/deepest/e.dng",
                "b.jpg"
            ]
        );
    }

    #[test]
    fn sizes_use_decimal_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1_500), "1.5 KB");
        assert_eq!(format_size(24_700_000), "24.7 MB");
        assert_eq!(format_size(3_200_000_000), "3.2 GB");
    }
}
