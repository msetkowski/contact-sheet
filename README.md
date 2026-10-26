# contact-sheet

A command-line tool for creating contact sheets from digital photos.

## What is a contact sheet?

A contact sheet (also called a *proof sheet*) is a page showing every frame of a
photo session as small images arranged in a grid, each one labelled so it can be
found again.

The term comes from film photography. Strips of developed negatives were laid
directly on a sheet of photographic paper, pressed flat under glass and exposed
to light. Because the negatives were in *contact* with the paper, every frame was
printed at its actual size. A whole roll, such as 36 frames of 35 mm film, fit on
one sheet. Photographers studied the sheet with a loupe, marked the best frames
with a grease pencil and chose which ones to enlarge. Contact sheets also show how
a photographer worked: the frames before and after "the" picture show the whole
process of getting it.

Digital photography got rid of negatives, but the need to see a whole session at a
glance did not go away. A digital contact sheet does the same job.

### What it is useful for

- **Selecting photos.** Compare similar frames side by side and pick the keepers
  before editing them one by one.
- **Proofing with clients and models.** Send or print one page instead of dozens of
  large files. The person reviewing can circle frames and refer to them by file name.
- **Archiving and cataloguing.** Print the sheets and keep them with a backup drive
  or with film negatives. They act as a visual index of what is stored where,
  readable without a computer.
- **Film scans.** Get an overview of a scanned roll (often large TIFF files) to check
  frame order, exposure and scanning quality.
- **Checking a shoot quickly.** Spot exposure jumps, missed focus or duplicates
  across a whole session in one view.
- **Portfolio and teaching.** Show the sequence that led to a final image, as
  photographers have long done with their film contact sheets.

This tool makes such sheets directly from RAW files (Canon, Nikon, Fujifilm, Hasselblad,
DNG), TIFF and JPEG, without importing them into a photo-management application. It produces
print-ready A4, A3 or Letter pages, or pages with a black background for viewing on screen.

## Example

A directory with Canon CR2 files, DNG panoramas, JPEG exports and 16-bit TIFF film scans:

```console
$ contact-sheet -v ~/Pictures/2026-10-17
Found 25 file(s)
  1G3A3760.CR2                      77.5 MB    6720×4480  Canon RAW (CR2)
  1G3A3764.jpg                      31.4 MB    9000×6000  JPEG
  1G3A3770.CR2                      85.7 MB    4480×6720  Canon RAW (CR2)
  1G3A3771-Pano.dng                 27.1 MB    7812×6894  DNG
  …
  MERCURY-MURNAU-100-70mm-01.tif   166.0 MB    5260×5259  TIFF
  …
Saved ~/Pictures/2026-10-17/contact-sheet/contact01.jpg (25 photo(s))

$ contact-sheet --background black ~/Pictures/2026-10-17
```

<p align="center">
  <img src="docs/example-white.jpg" width="49%" alt="Contact sheet with a white background, for printing">
  <img src="docs/example-black.jpg" width="49%" alt="Contact sheet with a black background, for on-screen viewing">
</p>

The example images are scaled down to 150 DPI. The actual output is a 300 DPI A4 page
(2480×3508 px).

## Installation

Download the archive for your platform from the
[Releases](https://github.com/msetkowski/contact-sheet/releases) page:

| Platform | Archive |
|---|---|
| macOS (Apple silicon) | `contact-sheet-vX.Y.Z-aarch64-apple-darwin.tar.gz` |
| Linux (x86_64, static) | `contact-sheet-vX.Y.Z-x86_64-unknown-linux-musl.tar.gz` |
| Linux (ARM64, static) | `contact-sheet-vX.Y.Z-aarch64-unknown-linux-musl.tar.gz` |
| Windows (x86_64) | `contact-sheet-vX.Y.Z-x86_64-pc-windows-msvc.zip` |

Unpack it and put `contact-sheet` (`contact-sheet.exe` on Windows) somewhere on your `PATH`.
Each archive has a `.sha256` checksum file next to it.

The macOS binaries are not signed, so Gatekeeper blocks them on first run. To allow
the downloaded binary to run, remove the quarantine attribute:

```sh
xattr -d com.apple.quarantine contact-sheet
```

To build from source instead, see [Building](#building).

## Usage

```sh
contact-sheet <DIRECTORY> [--recursive [DEPTH]] [--only FORMAT,…] [--paper a4|a3|letter] [--columns 5] [--dpi 300] [--format jpg|pdf] [--output-profile FILE.icc] [--max-pdf-size 500] [--max-pages N] [--quality 90] [--landscape] [--background white|black] [--no-labels] [--verbose]
```

The program scans the given directory and writes the sheets
`contact01.jpg`, `contact02.jpg`, … to the `<DIRECTORY>/contact-sheet` subdirectory.
Sheets from a previous run (`contactNN.jpg`) are removed first.
With `--format pdf` all pages go into a single file, `contact.pdf`, instead, or into
several files when the output is very large (see [PDF output](#pdf-output)).

- only the given directory is scanned by default; `--recursive` also scans all of its
  subdirectories, and `--recursive 2` goes at most two levels down. Hidden directories
  and `contact-sheet` output directories from earlier runs are skipped,
- `--only` limits the sheet to some formats – handy when the camera saves RAW + JPEG
  and every photo should appear once: `--only raw` (all RAW formats and DNG),
  `--only jpg` (`.jpg` and `.jpeg`), `--only tif`, or single RAW extensions such as
  `--only cr3`. Several formats can be listed (`--only cr3,dng`) and case does not
  matter. The program reports how many files the filter left out,
- paper size: `a4` (default), `a3` or `letter` (`--paper A3` works too),
- 300 DPI by default (2480×3508 px on A4, density stored in the JPEG), at most 600 DPI,
- 5 columns of thumbnails, each with the source file name below it (long names are
  shortened in the middle); with `--recursive` the label is the path relative to the
  scanned directory, e.g. `2026-10-17/IMG_0001.CR2`,
- photos are sorted by name (by relative path with `--recursive`), and the EXIF
  orientation is applied,
- row height adapts to the tallest thumbnail in the row; when the next row
  does not fit entirely on the page, a new file is started,
- background colour: `white` (default, for printing) or `black` (for on-screen viewing,
  with white labels),
- `--no-labels` leaves out the file names, so the sheet shows thumbnails only (rows
  move closer together, so more of them fit on a page),
- `--verbose` prints the name, file size, pixel dimensions and format of every file found,
- files that cannot be read are skipped with a warning.

### PDF output

`--format pdf` writes one multi-page PDF file intended for print workflows. It follows
the PDF/X-4 standard (ISO 15930-7):

- each page has the exact paper size (MediaBox and TrimBox), holding the rendered sheet as a JPEG
  image at the chosen `--dpi` and `--quality`,
- the images are tagged with an embedded sRGB ICC profile, so they are colour-managed
  rather than device RGB,
- the output intent is **FOGRA39** (ISO Coated v2, offset printing on coated paper), the
  usual print condition in Europe, unless you choose another one (see below); the
  profile is embedded in the file,
- the document carries the metadata PDF/X requires (title, dates, XMP, trapping status).

The images stay in RGB; conversion to CMYK is left to the print shop's RIP, which is
normal practice for PDF/X-4.

#### Large output

Pages are written to disk as soon as they are rendered, so memory use stays the same
whether the PDF has 10 pages or 10,000 – useful with `--recursive` over a whole archive.

Very large PDFs open slowly or not at all in many viewers and print shop systems, so
the output is split into several files rather than producing one unusable file:

- `--max-pdf-size MB` – largest size of one file, **500 MB** by default,
- `--max-pages N` – largest number of pages in one file (no limit by default).

A single file is named `contact.pdf`; split output is named `contact-01.pdf`,
`contact-02.pdf`, … and the document titles say which part each file is. After the
first page the program estimates the total size and tells you when the output will be
split; it also warns if you allow files over 1 GB.

Files are written under temporary names and renamed only when all of them are
complete. PDF output from an earlier run (`contact.pdf`, `contact-NN.pdf`) is replaced
only then, so an interrupted run never leaves a broken PDF behind: the previous output
stays intact, and a leftover hidden `.contact-NN.pdf.part` file is removed by the next
run. JPEG sheets from earlier runs are left in place.

To prepare the PDF for a different printing condition, pass the ICC profile your print
shop asks for, e.g. FOGRA51 (PSO Coated v3) or an uncoated-paper profile:

```sh
contact-sheet --format pdf --output-profile PSOcoated_v3.icc ~/Pictures/2026-10-17
```

The profile is checked before any photo is loaded. PDF/X requires an *output* (printer)
profile in CMYK, RGB or Gray, ICC version 2 or 4; display profiles such as sRGB or
Adobe RGB are rejected. The profile's description (e.g. "PSO Coated v3") is recorded
as the output condition.

## Supported formats

| Format | Extensions | Decoder |
|---|---|---|
| Canon RAW | `.cr2`, `.cr3`, `.crw` | [rawler](https://crates.io/crates/rawler) |
| Nikon RAW | `.nef`, `.nrw` | rawler |
| Fujifilm RAW (Bayer and X-Trans sensors) | `.raf` | rawler |
| Hasselblad RAW | `.3fr`, `.fff` | rawler |
| DNG | `.dng` | rawler |
| TIFF (uncompressed, also LZW/Deflate, 8/16 bit) | `.tif`, `.tiff` | [image](https://crates.io/crates/image) |
| JPEG | `.jpg`, `.jpeg` | image |

For RAW files the embedded JPEG preview is used (fast). If it is missing or
smaller than the thumbnail, the sensor data is developed by rawler.

Nikon *High Efficiency* NEF files (Z 8, Z 9 and similar) are shown using their embedded
preview. rawler cannot decode their sensor data yet, so `--verbose` prints `?` instead of
their pixel dimensions.

Hasselblad files from cameras rawler does not know (CFV-50c, CFV 100C, 907X and others,
including CFV backs whose model name carries the body, e.g. "CFV-50c/500") are shown
using the preview embedded in the file. Their sensor data cannot be decoded, so
`--verbose` prints `?` instead of their pixel dimensions.

## Building

Requires Rust (2024 edition). The project is pure Rust (no C libraries),
so it builds the same way on macOS, Linux and Windows:

```sh
cargo build --release
# output: target/release/contact-sheet (contact-sheet.exe on Windows)
```

## License

This project is licensed under the [Apache License, Version 2.0](LICENSE).

The embedded font `assets/DejaVuSans.ttf` comes from the
[DejaVu Fonts](https://dejavu-fonts.github.io/) project and is distributed under its own
license (Bitstream Vera / Arev, DejaVu changes in the public domain); see
[`assets/LICENSE-DejaVu.txt`](assets/LICENSE-DejaVu.txt).

The ICC profiles embedded in PDF output are in the public domain (CC0):
`assets/sRGB-v2-magic.icc` from [Compact ICC Profiles](https://github.com/saucecontrol/Compact-ICC-Profiles)
and `assets/FOGRA39L_coated.icc` from [colord](https://github.com/hughsie/colord)
(generated with ArgyllCMS from the FOGRA39 characterization data).
