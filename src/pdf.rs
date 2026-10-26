//! Writing contact sheets into PDF/X-4 files.
//!
//! Every page is the rendered sheet embedded as one JPEG image covering the
//! whole page. The image is tagged with an sRGB ICC profile and the document
//! carries an output intent – FOGRA39 (ISO Coated) unless the user supplies
//! their own ICC profile – so a print workflow knows how to convert it.

use std::borrow::Cow;
use std::fs::{self, File};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use pdf_writer::types::{OutputIntentSubtype, TrappingStatus};
use pdf_writer::writers::{Catalog, DocumentInfo};
use pdf_writer::{Chunk, Content, Date, Filter, Finish, Name, Rect, Ref, Str, TextStr};

use crate::output::OutputNames;

/// sRGB IEC 61966-2.1 (ICC v2, Compact ICC Profiles, CC0).
const SRGB_ICC: &[u8] = include_bytes!("../assets/sRGB-v2-magic.icc");
/// FOGRA39L Coated (ICC v2 output profile from colord, CC0).
const FOGRA39_ICC: &[u8] = include_bytes!("../assets/FOGRA39L_coated.icc");

/// The printing condition a PDF/X document is prepared for: an ICC output
/// profile plus the names PDF/X records alongside it.
#[derive(Debug, Clone)]
pub struct OutputIntent {
    profile: Cow<'static, [u8]>,
    /// Number of colour components of the profile (1 Gray, 3 RGB, 4 CMYK).
    components: u8,
    identifier: String,
    condition: Option<String>,
    registry: Option<&'static str>,
    info: String,
}

impl OutputIntent {
    /// The built-in default: FOGRA39, a characterized condition registered at color.org.
    pub fn fogra39() -> Self {
        Self {
            profile: Cow::Borrowed(FOGRA39_ICC),
            components: 4,
            identifier: "FOGRA39".to_string(),
            condition: Some(
                "Offset printing, according to ISO 12647-2:2004/Amd 1, OFCOM, \
                 paper type 1 or 2 = coated art, 115 g/m2"
                    .to_string(),
            ),
            registry: Some("http://www.color.org"),
            info: "Coated FOGRA39 (ISO 12647-2:2004)".to_string(),
        }
    }

    /// Loads a user-supplied ICC output profile, e.g. one provided by a print shop.
    pub fn from_icc_file(path: &Path) -> Result<Self> {
        let profile = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
        let fallback = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self::from_icc(profile, &fallback).with_context(|| {
            format!(
                "{} cannot be used as a PDF/X output profile",
                path.display()
            )
        })
    }

    fn from_icc(profile: Vec<u8>, fallback_name: &str) -> Result<Self> {
        let header = IccHeader::parse(&profile)?;
        ensure!(
            header.class == *b"prtr",
            "it is {} profile; PDF/X needs an output (printer) profile",
            describe_class(header.class)
        );
        ensure!(
            matches!(header.version_major, 2 | 4),
            "ICC version {} is not supported by PDF/X-4 (version 2 or 4 is needed)",
            header.version_major
        );
        let components = match &header.color_space {
            b"CMYK" => 4,
            b"RGB " => 3,
            b"GRAY" => 1,
            other => bail!(
                "its colour space '{}' is not supported (CMYK, RGB or Gray is needed)",
                String::from_utf8_lossy(other).trim()
            ),
        };
        let name = icc_description(&profile)
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| fallback_name.to_string());
        Ok(Self {
            profile: Cow::Owned(profile),
            components,
            identifier: name.clone(),
            condition: None,
            registry: None,
            info: name,
        })
    }

    /// Human-readable name of the printing condition.
    pub fn name(&self) -> &str {
        &self.info
    }
}

/// The fields of an ICC profile header that decide whether it suits PDF/X.
struct IccHeader {
    version_major: u8,
    class: [u8; 4],
    color_space: [u8; 4],
}

impl IccHeader {
    fn parse(data: &[u8]) -> Result<Self> {
        ensure!(
            data.len() >= 132 && data[36..40] == *b"acsp",
            "it is not an ICC profile"
        );
        let declared = u32::from_be_bytes(data[0..4].try_into().unwrap()) as usize;
        ensure!(declared <= data.len(), "the ICC profile is truncated");
        Ok(Self {
            version_major: data[8],
            class: data[12..16].try_into().unwrap(),
            color_space: data[16..20].try_into().unwrap(),
        })
    }
}

fn describe_class(class: [u8; 4]) -> String {
    match &class {
        b"mntr" => "a display".to_string(),
        b"scnr" => "an input (scanner/camera)".to_string(),
        b"spac" => "a colour space".to_string(),
        b"link" => "a device link".to_string(),
        b"abst" => "an abstract".to_string(),
        b"nmcl" => "a named colour".to_string(),
        other => format!("a '{}'", String::from_utf8_lossy(other)),
    }
}

/// Reads the profile description ('desc' tag): ICC v2 textDescriptionType or
/// ICC v4 multiLocalizedUnicodeType (English record preferred).
fn icc_description(data: &[u8]) -> Option<String> {
    let tag_count = be32(data, 128)?.min(1000);
    let entry = (0..tag_count)
        .map(|i| 132 + i * 12)
        .find(|&e| data.get(e..e + 4) == Some(b"desc"))?;
    let (offset, size) = (be32(data, entry + 4)?, be32(data, entry + 8)?);
    let tag = data.get(offset..offset.checked_add(size)?)?;

    let text = match tag.get(0..4)? {
        b"desc" => {
            let len = be32(tag, 8)?;
            String::from_utf8_lossy(tag.get(12..12 + len)?).into_owned()
        }
        b"mluc" => {
            let (records, record_size) = (be32(tag, 8)?.min(1000), be32(tag, 12)?);
            let record = |i: usize| 16 + i * record_size;
            let chosen = (0..records)
                .find(|&i| tag.get(record(i)..record(i) + 2) == Some(b"en"))
                .unwrap_or(0);
            let (len, start) = (
                be32(tag, record(chosen) + 4)?,
                be32(tag, record(chosen) + 8)?,
            );
            let (units, _) = tag.get(start..start + len)?.as_chunks::<2>();
            let units: Vec<u16> = units.iter().map(|&c| u16::from_be_bytes(c)).collect();
            String::from_utf16_lossy(&units)
        }
        _ => return None,
    };
    Some(text.trim_end_matches('\0').trim().to_string())
}

/// Reads a big-endian u32 at `at`, if the data is long enough.
fn be32(data: &[u8], at: usize) -> Option<usize> {
    let bytes = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes(bytes.try_into().ok()?) as usize)
}

/// Default size limit of a single PDF file; larger output is split into several files.
pub const DEFAULT_MAX_PDF_BYTES: u64 = 500_000_000;

/// Writes contact sheets into one or more PDF/X-4 files in a directory.
///
/// Pages are streamed to disk as they are rendered, so memory use does not
/// grow with the number of pages. A new file is started when the current one
/// would grow beyond `max_bytes` or already holds `max_pages` pages – several
/// usable files are better than one that viewers and RIPs cannot open.
///
/// Files are written under temporary names and renamed only once all of them
/// are complete: `contact.pdf` for a single file, otherwise `contact-01.pdf`,
/// `contact-02.pdf`, … (with the chosen base name). An interrupted run leaves
/// no partial PDF behind.
pub struct PdfSeries {
    dir: PathBuf,
    names: OutputNames,
    title: String,
    intent: Rc<OutputIntent>,
    max_pages: Option<usize>,
    max_bytes: u64,
    current: Option<PdfFile>,
    /// Completed files still under their temporary names, with their page counts.
    finished: Vec<(PathBuf, usize)>,
}

impl PdfSeries {
    pub fn new(
        dir: &Path,
        names: OutputNames,
        title: &str,
        intent: OutputIntent,
        max_pages: Option<usize>,
        max_bytes: u64,
    ) -> Result<Self> {
        remove_files_matching(dir, |f| names.is_pdf_temp(f))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            names,
            title: title.to_string(),
            intent: Rc::new(intent),
            max_pages,
            max_bytes,
            current: None,
            finished: Vec::new(),
        })
    }

    /// Adds a page of `size` (width, height in points) covered by `jpeg`, a
    /// baseline RGB JPEG of `width`×`height` px.
    pub fn add_page(
        &mut self,
        jpeg: &[u8],
        width: u32,
        height: u32,
        size: (f32, f32),
    ) -> Result<()> {
        if let Some(file) = &self.current {
            let full = self.max_pages.is_some_and(|max| file.page_count() >= max)
                || file.projected_size() + jpeg.len() as u64 > self.max_bytes;
            if full {
                self.close_current(true)?;
            }
        }
        let file = match &mut self.current {
            Some(file) => file,
            None => {
                let part = self.finished.len() + 1;
                let path = self.dir.join(self.names.pdf_temp(part));
                self.current
                    .insert(PdfFile::create(path, Rc::clone(&self.intent))?)
            }
        };
        file.add_page(jpeg, width, height, size)
    }

    /// Completes the open file. `split` tells whether the output has more than
    /// one file, which is then noted in the document title.
    fn close_current(&mut self, split: bool) -> Result<()> {
        let Some(mut file) = self.current.take() else {
            return Ok(());
        };
        let part = self.finished.len() + 1;
        let title = if split {
            format!("{}, part {part}", self.title)
        } else {
            self.title.clone()
        };
        file.close(&title)?;
        self.finished.push((file.path.clone(), file.page_count()));
        Ok(())
    }

    /// Completes the last file, replaces PDFs of an earlier run and gives the
    /// files their final names. Returns each file's path and page count.
    pub fn finish(mut self) -> Result<Vec<(PathBuf, usize)>> {
        let split = !self.finished.is_empty();
        self.close_current(split)?;
        let parts = std::mem::take(&mut self.finished);
        remove_files_matching(&self.dir, |f| self.names.is_pdf_output(f))?;

        let mut saved = Vec::with_capacity(parts.len());
        let count = parts.len();
        for (i, (temp, pages)) in parts.iter().enumerate() {
            let name = if count == 1 {
                self.names.pdf_single()
            } else {
                self.names.pdf_part(i + 1)
            };
            let path = self.dir.join(name);
            fs::rename(temp, &path).with_context(|| {
                format!("cannot rename {} to {}", temp.display(), path.display())
            })?;
            saved.push((path, *pages));
        }
        Ok(saved)
    }
}

impl Drop for PdfSeries {
    /// Removes completed files of a run that failed before `finish`.
    fn drop(&mut self) {
        for (temp, _) in &self.finished {
            let _ = fs::remove_file(temp);
        }
    }
}

fn remove_files_matching(dir: &Path, matches: impl Fn(&str) -> bool) -> Result<()> {
    for entry in fs::read_dir(dir)?.filter_map(|e| e.ok()) {
        if entry.file_name().to_str().is_some_and(&matches) {
            let path = entry.path();
            fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
        }
    }
    Ok(())
}

/// A single PDF/X-4 file written object by object.
///
/// Each indirect object is built in its own `Chunk`, so its byte offset is the
/// file position at which the chunk is written. Only these offsets stay in
/// memory; the cross-reference stream at the end is built from them.
struct PdfFile {
    /// `None` once the file is closed.
    out: Option<BufWriter<File>>,
    path: PathBuf,
    pos: u64,
    offsets: Vec<(Ref, u64)>,
    next_id: i32,
    catalog_id: Ref,
    page_tree_id: Ref,
    srgb_id: Ref,
    page_ids: Vec<Ref>,
    created: Timestamp,
    intent: Rc<OutputIntent>,
    done: bool,
}

impl PdfFile {
    fn create(path: PathBuf, intent: Rc<OutputIntent>) -> Result<Self> {
        let file =
            File::create(&path).with_context(|| format!("cannot create {}", path.display()))?;
        let mut pdf = Self {
            out: Some(BufWriter::new(file)),
            path,
            pos: 0,
            offsets: Vec::new(),
            next_id: 1,
            catalog_id: Ref::new(1),
            page_tree_id: Ref::new(1),
            srgb_id: Ref::new(1),
            page_ids: Vec::new(),
            created: Timestamp::now(),
            intent,
            done: false,
        };
        pdf.catalog_id = pdf.alloc();
        pdf.page_tree_id = pdf.alloc();
        pdf.srgb_id = pdf.alloc();

        // PDF/X-4 is based on PDF 1.6; the second line marks the file as binary.
        pdf.write_raw(b"%PDF-1.6\n%\x80\x80\x80\x80\n")?;
        let mut chunk = Chunk::new();
        chunk
            .icc_profile(pdf.srgb_id, SRGB_ICC)
            .n(3)
            .alternate()
            .device_rgb();
        pdf.write_object(&chunk)?;
        Ok(pdf)
    }

    fn alloc(&mut self) -> Ref {
        let id = Ref::new(self.next_id);
        self.next_id += 1;
        id
    }

    fn page_count(&self) -> usize {
        self.page_ids.len()
    }

    /// Size of the file once finished: what is written so far, plus the
    /// output profile and a margin for the closing objects.
    fn projected_size(&self) -> u64 {
        self.pos + self.intent.profile.len() as u64 + 64 * 1024
    }

    fn write_raw(&mut self, bytes: &[u8]) -> Result<()> {
        let out = self.out.as_mut().expect("PDF file already closed");
        out.write_all(bytes)
            .with_context(|| format!("failed to write {}", self.path.display()))?;
        self.pos += bytes.len() as u64;
        Ok(())
    }

    /// Writes a chunk holding exactly one indirect object.
    fn write_object(&mut self, chunk: &Chunk) -> Result<()> {
        let mut refs = chunk.refs();
        let id = refs.next().expect("chunk without an object");
        debug_assert!(refs.next().is_none(), "chunk with more than one object");
        self.offsets.push((id, self.pos));
        self.write_raw(chunk.as_bytes())
    }

    fn add_page(&mut self, jpeg: &[u8], width: u32, height: u32, size: (f32, f32)) -> Result<()> {
        let page_id = self.alloc();
        let image_id = self.alloc();
        let content_id = self.alloc();
        let (w, h) = size;
        let page_box = Rect::new(0.0, 0.0, w, h);

        let mut chunk = Chunk::new();
        let mut image = chunk.image_xobject(image_id, jpeg);
        image.filter(Filter::DctDecode);
        image.width(width as i32);
        image.height(height as i32);
        image.color_space().icc_based(self.srgb_id);
        image.bits_per_component(8);
        image.finish();
        self.write_object(&chunk)?;

        let mut content = Content::new();
        content
            .save_state()
            .transform([w, 0.0, 0.0, h, 0.0, 0.0])
            .x_object(Name(b"Im1"))
            .restore_state();
        let mut chunk = Chunk::new();
        chunk.stream(content_id, &content.finish());
        self.write_object(&chunk)?;

        let mut chunk = Chunk::new();
        let mut page = chunk.page(page_id);
        page.parent(self.page_tree_id)
            .media_box(page_box)
            .trim_box(page_box)
            .contents(content_id);
        page.resources().x_objects().pair(Name(b"Im1"), image_id);
        page.finish();
        self.write_object(&chunk)?;

        self.page_ids.push(page_id);
        Ok(())
    }

    /// Writes the document structure, metadata and cross-reference stream,
    /// and closes the file.
    fn close(&mut self, title: &str) -> Result<()> {
        let info_id = self.alloc();
        let metadata_id = self.alloc();
        let output_profile_id = self.alloc();
        let document_id = self.created.document_id(title);

        let mut chunk = Chunk::new();
        chunk
            .pages(self.page_tree_id)
            .kids(self.page_ids.iter().copied())
            .count(self.page_ids.len() as i32);
        self.write_object(&chunk)?;

        // Print shop profiles are often 1–2 MB and compress well.
        let intent = Rc::clone(&self.intent);
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&intent.profile, 6);
        let mut chunk = Chunk::new();
        let mut profile = chunk.icc_profile(output_profile_id, &compressed);
        profile.filter(Filter::FlateDecode);
        profile.n(i32::from(intent.components));
        match intent.components {
            1 => profile.alternate().device_gray(),
            3 => profile.alternate().device_rgb(),
            _ => profile.alternate().device_cmyk(),
        }
        profile.finish();
        self.write_object(&chunk)?;

        let producer = format!("contact-sheet {}", env!("CARGO_PKG_VERSION"));
        let date = self.created.pdf_date();
        let mut chunk = Chunk::new();
        chunk
            .indirect(info_id)
            .start::<DocumentInfo>()
            .title(TextStr(title))
            .creator(TextStr(&producer))
            .producer(TextStr(&producer))
            .creation_date(date)
            .modified_date(date)
            .trapped(TrappingStatus::NotTrapped);
        self.write_object(&chunk)?;

        let xmp = xmp_metadata(title, &producer, &self.created.iso8601(), &document_id);
        let mut chunk = Chunk::new();
        chunk.metadata(metadata_id, xmp.as_bytes());
        self.write_object(&chunk)?;

        let mut chunk = Chunk::new();
        let mut catalog = chunk.indirect(self.catalog_id).start::<Catalog>();
        catalog.pages(self.page_tree_id).metadata(metadata_id);
        let mut intents = catalog.output_intents();
        let mut output_intent = intents.push();
        output_intent
            .subtype(OutputIntentSubtype::PDFX)
            .output_condition_identifier(TextStr(&intent.identifier))
            .info(TextStr(&intent.info))
            .dest_output_profile(output_profile_id);
        if let Some(registry) = intent.registry {
            output_intent.registry_name(TextStr(registry));
        }
        if let Some(condition) = &intent.condition {
            output_intent.output_condition(TextStr(condition));
        }
        output_intent.finish();
        intents.finish();
        catalog.finish();
        self.write_object(&chunk)?;

        self.write_xref_stream(info_id, document_id.as_bytes())?;
        let mut out = self.out.take().expect("PDF file already closed");
        out.flush()
            .with_context(|| format!("failed to write {}", self.path.display()))?;
        self.done = true;
        Ok(())
    }

    /// Writes the cross-reference stream (PDF 1.5+, allowed by PDF/X-4) and
    /// the trailer. Unlike a classic xref table, whose offsets have 10 digits,
    /// it works for files of any size.
    fn write_xref_stream(&mut self, info_id: Ref, file_id: &[u8]) -> Result<()> {
        const WIDTHS: [usize; 3] = [1, 8, 2];
        let xref_id = self.alloc();
        let xref_pos = self.pos;
        self.offsets.push((xref_id, xref_pos));

        // Entry per object number: type 1 (in use) with its offset, type 0 for
        // object 0 and any unused number.
        let size = self.next_id as usize;
        let mut entries = vec![[0u8; 11]; size];
        entries[0] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff];
        for &(id, offset) in &self.offsets {
            let entry = &mut entries[id.get() as usize];
            entry[0] = 1;
            entry[1..9].copy_from_slice(&offset.to_be_bytes());
        }
        let data = entries.concat();

        let mut chunk = Chunk::new();
        let mut xref = chunk.stream(xref_id, &data);
        xref.pair(Name(b"Type"), Name(b"XRef"))
            .pair(Name(b"Size"), size as i32)
            .pair(Name(b"Root"), self.catalog_id)
            .pair(Name(b"Info"), info_id);
        xref.insert(Name(b"W"))
            .array()
            .items(WIDTHS.map(|w| w as i32));
        xref.insert(Name(b"ID"))
            .array()
            .items([Str(file_id), Str(file_id)]);
        xref.finish();
        self.write_raw(chunk.as_bytes())?;
        self.write_raw(format!("startxref\n{xref_pos}\n%%EOF\n").as_bytes())
    }
}

impl Drop for PdfFile {
    /// Removes the partial file of a run that failed or was cut short.
    fn drop(&mut self) {
        if !self.done {
            drop(self.out.take());
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// XMP packet with the entries PDF/X-4 requires (ISO 15930-7, 6.7.3).
fn xmp_metadata(title: &str, producer: &str, date: &str, document_id: &str) -> String {
    let title = xml_escape(title);
    let bom = '\u{feff}';
    format!(
        r#"<?xpacket begin="{bom}" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:pdf="http://ns.adobe.com/pdf/1.3/"
    xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"
    xmlns:pdfxid="http://www.npes.org/pdfx/ns/id/">
   <dc:format>application/pdf</dc:format>
   <dc:title><rdf:Alt><rdf:li xml:lang="x-default">{title}</rdf:li></rdf:Alt></dc:title>
   <xmp:CreateDate>{date}</xmp:CreateDate>
   <xmp:ModifyDate>{date}</xmp:ModifyDate>
   <xmp:MetadataDate>{date}</xmp:MetadataDate>
   <xmp:CreatorTool>{producer}</xmp:CreatorTool>
   <pdf:Producer>{producer}</pdf:Producer>
   <pdf:Trapped>False</pdf:Trapped>
   <xmpMM:DocumentID>uuid:{document_id}</xmpMM:DocumentID>
   <xmpMM:InstanceID>uuid:{document_id}</xmpMM:InstanceID>
   <xmpMM:VersionID>1</xmpMM:VersionID>
   <xmpMM:RenditionClass>default</xmpMM:RenditionClass>
   <pdfxid:GTS_PDFXVersion>PDF/X-4</pdfxid:GTS_PDFXVersion>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#
    )
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A UTC point in time, split into calendar fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Timestamp {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    nanos: u32,
}

impl Timestamp {
    fn now() -> Self {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Self::from_unix(since_epoch.as_secs(), since_epoch.subsec_nanos())
    }

    fn from_unix(secs: u64, nanos: u32) -> Self {
        let (days, rem) = (secs / 86_400, secs % 86_400);
        // Days to civil date (Howard Hinnant's algorithm), valid from 1970 on.
        let z = days + 719_468;
        let era = z / 146_097;
        let doe = z % 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + u64::from(month <= 2);
        Self {
            year: year as u16,
            month: month as u8,
            day: day as u8,
            hour: (rem / 3600) as u8,
            minute: (rem % 3600 / 60) as u8,
            second: (rem % 60) as u8,
            nanos,
        }
    }

    fn pdf_date(&self) -> Date {
        Date::new(self.year)
            .month(self.month)
            .day(self.day)
            .hour(self.hour)
            .minute(self.minute)
            .second(self.second)
            .utc_offset_hour(0)
            .utc_offset_minute(0)
    }

    fn iso8601(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// A UUID-formatted identifier, unique per run, for the XMP and trailer IDs.
    fn document_id(&self, title: &str) -> String {
        let hash = |seed: u64| {
            let mut h = DefaultHasher::new();
            (seed, self, title, std::process::id()).hash(&mut h);
            Hasher::finish(&h)
        };
        let id = (u128::from(hash(1)) << 64) | u128::from(hash(2));
        let hex = format!("{id:032x}");
        format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A4_PT: (f32, f32) = (595.2756, 841.8898);

    fn tiny_jpeg() -> Vec<u8> {
        use image::ImageEncoder;
        let mut buf = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut buf)
            .write_image(&[200; 8 * 8 * 3], 8, 8, image::ExtendedColorType::Rgb8)
            .unwrap();
        buf
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("contact-sheet-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    fn find(data: &[u8], needle: &[u8], from: usize) -> usize {
        from + data[from..]
            .windows(needle.len())
            .position(|w| w == needle)
            .unwrap_or_else(|| panic!("{:?} not found", String::from_utf8_lossy(needle)))
    }

    /// Checks the file structure and returns its page count: the trailer points
    /// to the xref stream, and every entry in it points to its object.
    fn check_pdf(path: &Path) -> usize {
        let data = fs::read(path).unwrap();
        assert!(data.starts_with(b"%PDF-1.6\n"));
        assert!(data.ends_with(b"%%EOF\n"));

        let start = find(&data, b"startxref\n", data.len() - 40) + b"startxref\n".len();
        let end = find(&data, b"\n", start);
        let xref_pos: usize = std::str::from_utf8(&data[start..end])
            .unwrap()
            .parse()
            .unwrap();
        let xref_header = &data[xref_pos..find(&data, b"\n", xref_pos)];
        assert!(
            xref_header.ends_with(b" 0 obj"),
            "startxref points to an object"
        );
        let dict_end = find(&data, b"stream\n", xref_pos);
        let dict = String::from_utf8_lossy(&data[xref_pos..dict_end]);
        assert!(dict.contains("/Type /XRef"), "{dict}");
        let size: usize = dict
            .split("/Size ")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .unwrap()
            .parse()
            .unwrap();

        let entries = &data[dict_end + b"stream\n".len()..][..size * 11];
        let mut in_use = 0;
        for (id, entry) in entries.as_chunks::<11>().0.iter().enumerate() {
            if entry[0] == 1 {
                let offset = u64::from_be_bytes(entry[1..9].try_into().unwrap()) as usize;
                let header = format!("{id} 0 obj");
                assert!(data[offset..].starts_with(header.as_bytes()), "object {id}");
                in_use += 1;
            }
        }
        assert_eq!(in_use, size - 1, "every object number is used");

        let text = String::from_utf8_lossy(&data);
        let count = text.split("/Count ").nth(1).unwrap();
        count.split_whitespace().next().unwrap().parse().unwrap()
    }

    fn series(dir: &Path, max_pages: Option<usize>, max_bytes: u64) -> PdfSeries {
        PdfSeries::new(
            dir,
            OutputNames::new(crate::output::DEFAULT_NAME).unwrap(),
            "Test",
            OutputIntent::fogra39(),
            max_pages,
            max_bytes,
        )
        .unwrap()
    }

    #[test]
    fn single_file_is_named_contact_pdf() {
        let dir = temp_dir("pdf-single");
        fs::write(dir.join("contact-01.pdf"), b"old").unwrap();
        fs::write(dir.join(".contact-01.pdf.part"), b"left by a killed run").unwrap();
        let jpeg = tiny_jpeg();
        let mut pdf = series(&dir, None, DEFAULT_MAX_PDF_BYTES);
        for _ in 0..3 {
            pdf.add_page(&jpeg, 8, 8, A4_PT).unwrap();
        }
        let saved = pdf.finish().unwrap();

        assert_eq!(saved, [(dir.join("contact.pdf"), 3)]);
        assert_eq!(names_in(&dir), ["contact.pdf"]);
        assert_eq!(check_pdf(&dir.join("contact.pdf")), 3);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn output_is_split_by_page_count() {
        let dir = temp_dir("pdf-pages");
        fs::write(dir.join("contact.pdf"), b"old").unwrap();
        let jpeg = tiny_jpeg();
        let mut pdf = series(&dir, Some(2), DEFAULT_MAX_PDF_BYTES);
        for _ in 0..5 {
            pdf.add_page(&jpeg, 8, 8, A4_PT).unwrap();
        }
        let saved = pdf.finish().unwrap();

        let pages: Vec<usize> = saved.iter().map(|(_, n)| *n).collect();
        assert_eq!(pages, [2, 2, 1]);
        assert_eq!(
            names_in(&dir),
            ["contact-01.pdf", "contact-02.pdf", "contact-03.pdf"]
        );
        for (path, pages) in &saved {
            assert_eq!(check_pdf(path), *pages);
        }
        let first = fs::read(dir.join("contact-01.pdf")).unwrap();
        assert!(String::from_utf8_lossy(&first).contains("Test, part 1"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn output_is_split_by_size() {
        let dir = temp_dir("pdf-size");
        let jpeg = vec![0u8; 100_000];
        // Room for the output profile, the margin and two such pages.
        let limit = FOGRA39_ICC.len() as u64 + 64 * 1024 + 250_000;
        let mut pdf = series(&dir, None, limit);
        for _ in 0..5 {
            pdf.add_page(&jpeg, 8, 8, A4_PT).unwrap();
        }
        let saved = pdf.finish().unwrap();

        let pages: Vec<usize> = saved.iter().map(|(_, n)| *n).collect();
        assert_eq!(pages, [2, 2, 1]);
        for (path, _) in &saved {
            assert!(fs::metadata(path).unwrap().len() <= limit);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unfinished_output_is_removed() {
        let dir = temp_dir("pdf-drop");
        let jpeg = tiny_jpeg();
        let mut pdf = series(&dir, Some(1), DEFAULT_MAX_PDF_BYTES);
        for _ in 0..3 {
            pdf.add_page(&jpeg, 8, 8, A4_PT).unwrap();
        }
        drop(pdf);
        assert!(names_in(&dir).is_empty(), "{:?}", names_in(&dir));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unix_time_converts_to_calendar_date() {
        let t = Timestamp::from_unix(1_793_053_815, 0);
        assert_eq!(t.iso8601(), "2026-10-26T22:30:15Z");
        // Leap day.
        assert_eq!(
            Timestamp::from_unix(951_782_400, 0).iso8601(),
            "2000-02-29T00:00:00Z"
        );
    }

    #[test]
    fn printer_profile_is_accepted_as_output_intent() {
        let intent = OutputIntent::from_icc(FOGRA39_ICC.to_vec(), "fallback").unwrap();
        assert_eq!(intent.components, 4);
        assert_eq!(intent.name(), "FOGRA39L Coated");
        assert_eq!(intent.identifier, "FOGRA39L Coated");
        assert_eq!(intent.registry, None);
    }

    #[test]
    fn display_profile_is_rejected() {
        let err = OutputIntent::from_icc(SRGB_ICC.to_vec(), "sRGB").unwrap_err();
        assert!(err.to_string().contains("display"), "{err}");
    }

    #[test]
    fn non_icc_file_is_rejected() {
        let err = OutputIntent::from_icc(b"not a profile".to_vec(), "x").unwrap_err();
        assert!(err.to_string().contains("not an ICC profile"), "{err}");
    }

    #[test]
    fn v4_description_prefers_english() {
        // Minimal ICC v4 Gray printer profile with a two-language 'desc' (mluc) tag.
        let utf16 = |s: &str| {
            s.encode_utf16()
                .flat_map(u16::to_be_bytes)
                .collect::<Vec<u8>>()
        };
        let (pl, en) = (utf16("Profil drukarni"), utf16("Print shop profile"));
        let mut mluc = b"mluc\0\0\0\0".to_vec();
        mluc.extend_from_slice(&2u32.to_be_bytes()); // records
        mluc.extend_from_slice(&12u32.to_be_bytes()); // record size
        let strings_at = 16 + 2 * 12;
        for (lang, text, at) in [
            (b"plPL", &pl, strings_at),
            (b"enUS", &en, strings_at + pl.len()),
        ] {
            mluc.extend_from_slice(lang);
            mluc.extend_from_slice(&(text.len() as u32).to_be_bytes());
            mluc.extend_from_slice(&(at as u32).to_be_bytes());
        }
        mluc.extend_from_slice(&pl);
        mluc.extend_from_slice(&en);

        let mut icc = vec![0u8; 128];
        icc[8] = 4;
        icc[12..16].copy_from_slice(b"prtr");
        icc[16..20].copy_from_slice(b"GRAY");
        icc[36..40].copy_from_slice(b"acsp");
        icc.extend_from_slice(&1u32.to_be_bytes());
        icc.extend_from_slice(b"desc");
        icc.extend_from_slice(&144u32.to_be_bytes());
        icc.extend_from_slice(&(mluc.len() as u32).to_be_bytes());
        icc.extend_from_slice(&mluc);
        let size = icc.len() as u32;
        icc[0..4].copy_from_slice(&size.to_be_bytes());

        let intent = OutputIntent::from_icc(icc, "fallback").unwrap();
        assert_eq!(intent.components, 1);
        assert_eq!(intent.name(), "Print shop profile");
    }

    #[test]
    fn title_is_escaped_in_xmp() {
        let xmp = xmp_metadata("A & B <x>", "p", "d", "id");
        assert!(xmp.contains("A &amp; B &lt;x&gt;"));
    }
}
