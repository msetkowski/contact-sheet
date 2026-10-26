# contact-sheet

Print-ready contact sheets from RAW, TIFF and JPEG files, straight from the command line. No import, no catalogue, no photo-management app.

<p align="center">
  <a href="docs/example-white.jpg"><img src="docs/example-white.jpg" alt="A contact sheet of 25 photos: Canon RAW files, DNG panoramas, JPEG exports and 16-bit TIFF film scans" width="560"></a>
</p>

```
contact-sheet ~/Pictures/2026-10-17
```

- **Reads what photographers actually shoot:** Canon, Nikon, Fujifilm (incl. X-Trans), Hasselblad, DNG, 8/16-bit TIFF film scans and JPEG.
- **Built for print:** A4, A3 or Letter at up to 600 DPI, or PDF/X-4 with an embedded output intent (FOGRA39 by default, or your print shop's ICC profile).
- **Handles whole archives:** walks subfolders, shows each frame once in RAW+JPEG sessions, streams thousands of pages to disk with constant memory and never leaves a broken PDF behind.
- **One static binary** for macOS, Linux and Windows. Pure Rust, no dependencies to install.

## Recipes

**A scanned roll of film, ready to print**

```
contact-sheet ~/Scans/roll-042
```

**A RAW+JPEG session, every frame shown once**

```
contact-sheet --only raw ~/Pictures/2026-10-17
```

**Checking a shoot on screen**

```
contact-sheet --background black --landscape ~/Pictures/2026-10-17
```

<p align="center">
  <a href="docs/example-black.jpg"><img src="docs/example-black.jpg" alt="The same contact sheet with a black background, for on-screen viewing" width="420"></a>
</p>

**A whole archive as a print index, one PDF for the print shop**

```
contact-sheet --recursive --format pdf ~/Archive/2026
```

Labels show the path relative to the scanned folder (e.g. `2026-10-17/IMG_0001.CR2`), so every frame on paper can be found on disk.

**Prepared for a specific press or paper**

```
contact-sheet --format pdf --output-profile PSOcoated_v3.icc ~/Pictures/2026-10-17
```

**US Letter for a home printer**

```
contact-sheet --paper letter ~/Pictures/2026-10-17
```

## Installation

Download the archive for your platform from the [Releases](https://github.com/msetkowski/contact-sheet/releases) page:

| Platform               | Archive                                                  |
|------------------------|----------------------------------------------------------|
| macOS (Apple silicon)  | `contact-sheet-vX.Y.Z-aarch64-apple-darwin.tar.gz`       |
| macOS (Intel)          | `contact-sheet-vX.Y.Z-x86_64-apple-darwin.tar.gz`        |
| Linux (x86_64, static) | `contact-sheet-vX.Y.Z-x86_64-unknown-linux-musl.tar.gz`  |
| Linux (ARM64, static)  | `contact-sheet-vX.Y.Z-aarch64-unknown-linux-musl.tar.gz` |
| Windows (x86_64)       | `contact-sheet-vX.Y.Z-x86_64-pc-windows-msvc.zip`        |

Unpack it and put `contact-sheet` (`contact-sheet.exe` on Windows) somewhere on your `PATH`. Each archive has a `.sha256` checksum file next to it.

The macOS binaries are not signed, so Gatekeeper blocks them on first run. To allow the downloaded binary to run, remove the quarantine attribute:

```
xattr -d com.apple.quarantine contact-sheet
```

### With Cargo

With [cargo-binstall](https://github.com/cargo-bins/cargo-binstall), Cargo downloads the prebuilt binary from the GitHub release for your platform – no compiling:

```
cargo binstall contact-sheet
```

Or build it from [crates.io](https://crates.io/crates/contact-sheet) (Rust 1.89 or newer; this also works on platforms without a prebuilt binary):

```
cargo install contact-sheet
```

Both put `contact-sheet` into `~/.cargo/bin`, which is on your `PATH` after a standard Rust installation.

To build from a clone of the repository instead, see [Building](#building).

## Usage

```
contact-sheet [OPTIONS] <DIRECTORY>
```

Sheets are written to `<DIRECTORY>/contact-sheet/` as `contact01.jpg`, `contact02.jpg`, … With `--format pdf` all pages go into `contact.pdf` instead, or into several files when the output is very large (see [Large output](#large-output)). `--output-folder` and `--name` change the folder and the file names: `-o ~/Sheets -n wedding` writes `~/Sheets/wedding01.jpg` or `~/Sheets/wedding.pdf`.

Sheets of a previous run with the same name are replaced; surplus pages it had (e.g. `contact07.jpg` when there are now only six pages) are removed. Only names exactly as the program writes them count as its output – with `-n IMG_`, `IMG_07.jpg` is a sheet but `IMG_0007.jpg` is not – so photos in the output folder are never overwritten or deleted, and earlier sheets are never read as photos.

| Option | Default | What it does |
|---|---|---|
| `-o`, `--output-folder DIR` | `<DIRECTORY>/contact-sheet` | Folder for the sheets; created if needed. |
| `-n`, `--name NAME` | `contact` | Base name of the output files: `NAME01.jpg`, … or `NAME.pdf`. |
| `--recursive [DEPTH]` | off | Also scan subfolders; `--recursive 2` goes at most two levels down. Hidden folders and earlier `contact-sheet` output are skipped. |
| `--only FORMAT,…` | all | Include only some formats: `raw` (all RAW formats and DNG), `jpg`, `tif`, or single extensions such as `cr3`. Several can be listed (`--only cr3,dng`); case does not matter. |
| `--paper a4\|a3\|letter` | `a4` | Paper size. |
| `--landscape` | off | Landscape orientation. |
| `--columns N` | `5` | Thumbnails per row. |
| `--dpi N` | `300` | Resolution, at most 600 (300 DPI on A4 is 2480×3508 px; the density is stored in the JPEG). |
| `--format jpg\|pdf` | `jpg` | JPEG pages, or one PDF/X-4 file (see [PDF output](#pdf-output)). |
| `--quality N` | `90` | JPEG quality, also used for images inside the PDF. |
| `--background white\|black` | `white` | `white` for printing, `black` for on-screen viewing (with white labels). |
| `--no-labels` | off | Leave out file names; rows move closer together, so more fit on a page. |
| `--output-profile FILE.icc` | FOGRA39 | Output intent for PDF (see [PDF output](#pdf-output)). |
| `--max-pdf-size MB` | `500` | Largest size of one PDF file before the output is split. |
| `--max-pages N` | no limit | Largest number of pages in one PDF file. |
| `-v`, `--verbose` | off | Print the name, file size, pixel dimensions and format of every file found. |
| `--progress` | off | Print a line for every photo as it is processed: `[12/340] 2026-10-17/IMG_0001.CR2` – handy for progress bars in scripts and other programs. |

How a sheet is laid out:

- photos are sorted by name (by relative path with `--recursive`), and the EXIF orientation is applied,
- each thumbnail is labelled with its file name (long names are shortened in the middle); with `--recursive` the label is the path relative to the scanned folder,
- row height adapts to the tallest thumbnail in the row; when the next row does not fit entirely on the page, a new page is started,
- with `--only`, the program reports how many files the filter left out,
- files that cannot be read are skipped with a warning.

The run ends with a summary such as `Done: 118 photo(s) on 5 page(s), 2 file(s) skipped`. The exit status tells scripts how it went:

| Exit status | Meaning |
|---|---|
| `0` | Every photo is on the sheets. |
| `1` | Error, e.g. the folder cannot be read or no photo could be loaded; no sheets were written. |
| `2` | The sheets were written, but some files could not be read and were skipped (each is listed on stderr). |

### PDF output

`--format pdf` writes a multi-page PDF intended for print workflows. It follows the PDF/X-4 standard (ISO 15930-7):

- each page has the exact paper size (MediaBox and TrimBox), holding the rendered sheet as a JPEG image at the chosen `--dpi` and `--quality`,
- the images are tagged with an embedded sRGB ICC profile, so they are colour-managed rather than device RGB,
- the output intent is **FOGRA39** (ISO Coated v2, offset printing on coated paper), the usual print condition in Europe, unless you choose another one; the profile is embedded in the file,
- the document carries the metadata PDF/X requires (title, dates, XMP, trapping status).

The images stay in RGB; conversion to CMYK is left to the print shop's RIP, which is normal practice for PDF/X-4.

To prepare the PDF for a different printing condition, pass the ICC profile your print shop asks for, e.g. FOGRA51 (PSO Coated v3) or an uncoated-paper profile. The profile is checked before any photo is loaded. PDF/X requires an *output* (printer) profile in CMYK, RGB or Gray, ICC version 2 or 4; display profiles such as sRGB or Adobe RGB are rejected. The profile's description (e.g. "PSO Coated v3") is recorded as the output condition.

#### Large output

Pages are written to disk as soon as they are rendered, so memory use stays the same whether the PDF has 10 pages or 10,000 – useful with `--recursive` over a whole archive.

Very large PDFs open slowly or not at all in many viewers and print shop systems, so the output is split into several files rather than producing one unusable file (see `--max-pdf-size` and `--max-pages`). A single file is named `contact.pdf`; split output is named `contact-01.pdf`, `contact-02.pdf`, … and the document titles say which part each file is. After the first page the program estimates the total size and tells you when the output will be split; it also warns if you allow files over 1 GB.

Files are written under temporary names and renamed only when all of them are complete. PDF output from an earlier run (`contact.pdf`, `contact-NN.pdf`) is replaced only then, so an interrupted run never leaves a broken PDF behind: the previous output stays intact, and a leftover hidden `.contact-NN.pdf.part` file is removed by the next run. JPEG sheets from earlier runs are left in place. (With `--name`, these names use the chosen base name instead of `contact`.)

## Supported formats

| Format                                          | Extensions             | Decoder                                   |
|-------------------------------------------------|------------------------|-------------------------------------------|
| Canon RAW                                       | `.cr2`, `.cr3`, `.crw` | [rawler](https://crates.io/crates/rawler) |
| Nikon RAW                                       | `.nef`, `.nrw`         | rawler                                    |
| Fujifilm RAW (Bayer and X-Trans sensors)        | `.raf`                 | rawler                                    |
| Hasselblad RAW                                  | `.3fr`, `.fff`         | rawler                                    |
| DNG                                             | `.dng`                 | rawler                                    |
| TIFF (uncompressed, also LZW/Deflate, 8/16 bit) | `.tif`, `.tiff`        | [image](https://crates.io/crates/image)   |
| JPEG                                            | `.jpg`, `.jpeg`        | image                                     |

For RAW files the embedded JPEG preview is used (fast). If it is missing or smaller than the thumbnail, the sensor data is developed by rawler.

Large TIFF and JPEG files, such as high-resolution film scans of several GB, are supported. Images over 512 MiB once decoded are loaded one at a time and shrunk right away, so a folder full of them does not run out of memory; smaller files still load in parallel.

Nikon *High Efficiency* NEF files (Z 8, Z 9 and similar) are shown using their embedded preview. rawler cannot decode their sensor data yet, so `--verbose` prints `?` instead of their pixel dimensions.

Hasselblad files from cameras rawler does not know (CFV-50c, CFV 100C, 907X and others, including CFV backs whose model name carries the body, e.g. "CFV-50c/500") are shown using the preview embedded in the file. Their sensor data cannot be decoded, so `--verbose` prints `?` instead of their pixel dimensions.

## What is a contact sheet?

A contact sheet (also called a *proof sheet*) is a page showing every frame of a photo session as small images arranged in a grid, each one labelled so it can be found again.

The term comes from film photography. Strips of developed negatives were laid directly on a sheet of photographic paper, pressed flat under glass and exposed to light. Because the negatives were in *contact* with the paper, every frame was printed at its actual size. A whole roll, such as 36 frames of 35 mm film, fit on one sheet. Photographers studied the sheet with a loupe, marked the best frames with a grease pencil and chose which ones to enlarge. Contact sheets also show how a photographer worked: the frames before and after "the" picture show the whole process of getting it.

Digital photography got rid of negatives, but the need to see a whole session at a glance did not go away. A digital contact sheet does the same job.

### What it is useful for

- **Selecting photos.** Compare similar frames side by side and pick the keepers before editing them one by one.
- **Proofing with clients and models.** Send or print one page instead of dozens of large files. The person reviewing can circle frames and refer to them by file name.
- **Archiving and cataloguing.** Print the sheets and keep them with a backup drive or with film negatives. They act as a visual index of what is stored where, readable without a computer.
- **Film scans.** Get an overview of a scanned roll (often large TIFF files) to check frame order, exposure and scanning quality.
- **Checking a shoot quickly.** Spot exposure jumps, missed focus or duplicates across a whole session in one view.
- **Portfolio and teaching.** Show the sequence that led to a final image, as photographers have long done with their film contact sheets.

## Building

Requires Rust 1.89 or newer. The project is pure Rust (no C libraries), so it builds the same way on macOS, Linux and Windows:

```
cargo build --release
# output: target/release/contact-sheet (contact-sheet.exe on Windows)
```

## License

This project is licensed under the [Apache License, Version 2.0](LICENSE).

The embedded font `assets/DejaVuSans.ttf` comes from the [DejaVu Fonts](https://dejavu-fonts.github.io/) project and is distributed under its own license (Bitstream Vera / Arev, DejaVu changes in the public domain); see [`assets/LICENSE-DejaVu.txt`](assets/LICENSE-DejaVu.txt).

The binaries also contain the Rust libraries listed, with their licences, in [`THIRD-PARTY-LICENSES.txt`](THIRD-PARTY-LICENSES.txt), which is included in every release archive. Most are under permissive licences (MIT, Apache-2.0, BSD, Zlib); [rawler](https://github.com/dnglab/dnglab), which decodes the RAW files, is under the GNU LGPL 2.1 – the file says where to get its source code and how to build contact-sheet with a modified version. After changing dependencies, regenerate the file with `ruby scripts/third-party-licenses.rb`; the release workflow fails if it is out of date.

The ICC profiles embedded in PDF output are in the public domain (CC0): `assets/sRGB-v2-magic.icc` from [Compact ICC Profiles](https://github.com/saucecontrol/Compact-ICC-Profiles) and `assets/FOGRA39L_coated.icc` from [colord](https://github.com/hughsie/colord) (generated with ArgyllCMS from the FOGRA39 characterization data).
