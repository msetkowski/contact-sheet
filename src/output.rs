//! Names of the files a run writes, and recognising them again.
//!
//! Output from an earlier run is replaced, and output files lying among the
//! photos are not read as photos. Since the output folder may be any folder,
//! possibly full of photos, only names exactly as this program writes them
//! count as output: with the base name `IMG_`, `IMG_05.jpg` is a sheet but
//! `IMG_0005.jpg` is not.

use anyhow::{Result, bail};

pub const DEFAULT_NAME: &str = "contact";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputNames {
    base: String,
}

impl OutputNames {
    pub fn new(base: &str) -> Result<Self> {
        if base.is_empty() || base.chars().count() > 100 {
            bail!("the name must have 1 to 100 characters");
        }
        if let Some(c) = base
            .chars()
            .find(|c| c.is_control() || "/\\<>:\"|?*".contains(*c))
        {
            bail!("the name must not contain {c:?}; it is used as a file name");
        }
        if base.starts_with('.') || base.ends_with('.') || base.ends_with(' ') {
            bail!("the name must not start with '.' or end with '.' or a space");
        }
        Ok(Self {
            base: base.to_string(),
        })
    }

    /// Page `n` (from 1) of JPEG output: `contact01.jpg`.
    pub fn jpeg_page(&self, n: usize) -> String {
        format!("{}{n:02}.jpg", self.base)
    }

    /// The page number of a JPEG sheet file name, if it is one.
    pub fn jpeg_page_number(&self, file_name: &str) -> Option<usize> {
        file_name
            .strip_prefix(&self.base)?
            .strip_suffix(".jpg")
            .and_then(page_number)
    }

    /// PDF output that fits into one file: `contact.pdf`.
    pub fn pdf_single(&self) -> String {
        format!("{}.pdf", self.base)
    }

    /// Part `n` (from 1) of PDF output split into several files: `contact-01.pdf`.
    pub fn pdf_part(&self, n: usize) -> String {
        format!("{}-{n:02}.pdf", self.base)
    }

    /// Hidden temporary name of PDF part `n` while it is written.
    pub fn pdf_temp(&self, n: usize) -> String {
        format!(".{}-{n:02}.pdf.part", self.base)
    }

    pub fn is_pdf_output(&self, file_name: &str) -> bool {
        file_name == self.pdf_single()
            || file_name
                .strip_prefix(&self.base)
                .and_then(|r| r.strip_prefix('-'))
                .and_then(|r| r.strip_suffix(".pdf"))
                .and_then(page_number)
                .is_some()
    }

    pub fn is_pdf_temp(&self, file_name: &str) -> bool {
        file_name
            .strip_prefix('.')
            .and_then(|r| r.strip_prefix(&self.base))
            .and_then(|r| r.strip_prefix('-'))
            .and_then(|r| r.strip_suffix(".pdf.part"))
            .and_then(page_number)
            .is_some()
    }
}

/// A page number written as `{n:02}`: `07` and `123`, but not `7` or `007`.
fn page_number(digits: &str) -> Option<usize> {
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: usize = digits.parse().ok()?;
    (n > 0 && format!("{n:02}") == digits).then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_base_name() {
        let names = OutputNames::new("wedding").unwrap();
        assert_eq!(names.jpeg_page(1), "wedding01.jpg");
        assert_eq!(names.jpeg_page(123), "wedding123.jpg");
        assert_eq!(names.pdf_single(), "wedding.pdf");
        assert_eq!(names.pdf_part(2), "wedding-02.pdf");
        assert_eq!(names.pdf_temp(2), ".wedding-02.pdf.part");
    }

    #[test]
    fn only_exact_output_names_are_recognised() {
        let names = OutputNames::new("IMG_").unwrap();
        assert_eq!(names.jpeg_page_number("IMG_05.jpg"), Some(5));
        assert_eq!(names.jpeg_page_number("IMG_123.jpg"), Some(123));
        // Photos that merely look similar.
        assert_eq!(names.jpeg_page_number("IMG_0005.jpg"), None);
        assert_eq!(names.jpeg_page_number("IMG_5.jpg"), None);
        assert_eq!(names.jpeg_page_number("IMG_00.jpg"), None);
        assert_eq!(names.jpeg_page_number("IMG_05.JPG"), None);
        assert_eq!(names.jpeg_page_number("IMG_05a.jpg"), None);

        assert!(names.is_pdf_output("IMG_.pdf"));
        assert!(names.is_pdf_output("IMG_-03.pdf"));
        assert!(!names.is_pdf_output("IMG_-003.pdf"));
        assert!(!names.is_pdf_output("IMG_03.pdf"));
        assert!(names.is_pdf_temp(".IMG_-03.pdf.part"));
        assert!(!names.is_pdf_temp(".IMG_-3.pdf.part"));
    }

    #[test]
    fn invalid_names_are_rejected() {
        for bad in [
            "",
            "a/b",
            "a\\b",
            "a:b",
            ".hidden",
            "trailing.",
            "trailing ",
            "tab\t",
        ] {
            assert!(OutputNames::new(bad).is_err(), "{bad:?}");
        }
        assert!(OutputNames::new("Sesja ślubna 2026").is_ok());
    }
}
