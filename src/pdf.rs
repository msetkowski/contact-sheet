//! Writing contact sheets into a single PDF/X-4 file.
//!
//! Every page is the rendered sheet embedded as one JPEG image covering the
//! whole page. The image is tagged with an sRGB ICC profile and the document
//! carries an output intent – FOGRA39 (ISO Coated) unless the user supplies
//! their own ICC profile – so a print workflow knows how to convert it.

use std::borrow::Cow;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use pdf_writer::types::{OutputIntentSubtype, TrappingStatus};
use pdf_writer::{Content, Date, Filter, Finish, Name, Pdf, Rect, Ref, TextStr};

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

/// Builds a multi-page PDF/X-4 document in memory, one sheet at a time.
pub struct PdfSheets {
    pdf: Pdf,
    next_id: i32,
    catalog_id: Ref,
    page_tree_id: Ref,
    srgb_id: Ref,
    page_ids: Vec<Ref>,
    title: String,
    created: Timestamp,
    intent: OutputIntent,
}

impl PdfSheets {
    pub fn new(title: &str, intent: OutputIntent) -> Self {
        let mut sheets = Self {
            pdf: Pdf::new(),
            next_id: 1,
            catalog_id: Ref::new(1),
            page_tree_id: Ref::new(1),
            srgb_id: Ref::new(1),
            page_ids: Vec::new(),
            title: title.to_string(),
            created: Timestamp::now(),
            intent,
        };
        // PDF/X-4 is based on PDF 1.6.
        sheets.pdf.set_version(1, 6);
        sheets.catalog_id = sheets.alloc();
        sheets.page_tree_id = sheets.alloc();
        sheets.srgb_id = sheets.alloc();
        sheets
            .pdf
            .icc_profile(sheets.srgb_id, SRGB_ICC)
            .n(3)
            .alternate()
            .device_rgb();
        sheets
    }

    fn alloc(&mut self) -> Ref {
        let id = Ref::new(self.next_id);
        self.next_id += 1;
        id
    }

    /// Adds a page of `size` (width, height in points) covered by `jpeg`, a
    /// baseline RGB JPEG of `width`×`height` px.
    pub fn add_page(&mut self, jpeg: &[u8], width: u32, height: u32, size: (f32, f32)) {
        let page_id = self.alloc();
        let image_id = self.alloc();
        let content_id = self.alloc();
        let (w, h) = size;
        let page_box = Rect::new(0.0, 0.0, w, h);

        let mut image = self.pdf.image_xobject(image_id, jpeg);
        image.filter(Filter::DctDecode);
        image.width(width as i32);
        image.height(height as i32);
        image.color_space().icc_based(self.srgb_id);
        image.bits_per_component(8);
        image.finish();

        let mut content = Content::new();
        content
            .save_state()
            .transform([w, 0.0, 0.0, h, 0.0, 0.0])
            .x_object(Name(b"Im1"))
            .restore_state();
        self.pdf.stream(content_id, &content.finish());

        let mut page = self.pdf.page(page_id);
        page.parent(self.page_tree_id)
            .media_box(page_box)
            .trim_box(page_box)
            .contents(content_id);
        page.resources().x_objects().pair(Name(b"Im1"), image_id);
        page.finish();
        self.page_ids.push(page_id);
    }

    pub fn page_count(&self) -> usize {
        self.page_ids.len()
    }

    /// Writes the document structure and metadata and saves the file.
    pub fn save(mut self, path: &Path) -> Result<()> {
        let info_id = self.alloc();
        let metadata_id = self.alloc();
        let output_profile_id = self.alloc();
        let document_id = self.created.document_id(&self.title);

        self.pdf
            .pages(self.page_tree_id)
            .kids(self.page_ids.iter().copied())
            .count(self.page_ids.len() as i32);

        let intent = &self.intent;
        // Print shop profiles are often 1–2 MB and compress well.
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&intent.profile, 6);
        let mut profile = self.pdf.icc_profile(output_profile_id, &compressed);
        profile.filter(Filter::FlateDecode);
        profile.n(i32::from(intent.components));
        match intent.components {
            1 => profile.alternate().device_gray(),
            3 => profile.alternate().device_rgb(),
            _ => profile.alternate().device_cmyk(),
        }
        profile.finish();

        let producer = format!("contact-sheet {}", env!("CARGO_PKG_VERSION"));
        let date = self.created.pdf_date();
        self.pdf
            .document_info(info_id)
            .title(TextStr(&self.title))
            .creator(TextStr(&producer))
            .producer(TextStr(&producer))
            .creation_date(date)
            .modified_date(date)
            .trapped(TrappingStatus::NotTrapped);

        let xmp = xmp_metadata(
            &self.title,
            &producer,
            &self.created.iso8601(),
            &document_id,
        );
        self.pdf.metadata(metadata_id, xmp.as_bytes());

        let mut catalog = self.pdf.catalog(self.catalog_id);
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

        let id = document_id.as_bytes().to_vec();
        self.pdf.set_file_id((id.clone(), id));
        fs::write(path, self.pdf.finish())
            .with_context(|| format!("failed to write {}", path.display()))
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
