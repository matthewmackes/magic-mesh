//! Native-office admission for the embedded Editor (WL-FUNC-011 S6).
//!
//! The current production package does not contain a safe, compositor-free
//! LibreOfficeKit adapter.  Office containers must therefore never fall through
//! to the UTF-8 rope loader: doing so lossy-decodes ZIP/container bytes and a
//! subsequent save can destroy the document.  This module identifies the
//! office formats owned by S6, applies the file boundary that a future adapter
//! must inherit, and fails closed with the exact missing production component.

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

/// Largest office document admitted to a native session.
pub const MAX_OFFICE_DOCUMENT_BYTES: u64 = 256 * 1024 * 1024;

/// Office document families promised by WL-FUNC-011 S6.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfficeKind {
    /// Writer-style document.
    Document,
    /// Calc-style spreadsheet.
    Spreadsheet,
    /// Impress-style presentation.
    Presentation,
}

impl fmt::Display for OfficeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Document => "Document",
            Self::Spreadsheet => "Spreadsheet",
            Self::Presentation => "Presentation",
        })
    }
}

/// Return the native-office family for a path, case-insensitively.
#[must_use]
pub fn office_kind(path: &Path) -> Option<OfficeKind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "odt" | "doc" | "docx" | "rtf" => Some(OfficeKind::Document),
        "ods" | "xls" | "xlsx" => Some(OfficeKind::Spreadsheet),
        "odp" | "ppt" | "pptx" => Some(OfficeKind::Presentation),
        _ => None,
    }
}

/// Admit an office path to the production native-session boundary.
///
/// This deliberately returns an error after validating the input because the
/// required safe out-of-process LibreOfficeKit adapter is not yet packaged.
/// It is a real fail-closed boundary, not a connected/session placeholder.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidInput`] for non-regular or oversized input,
/// an underlying metadata error for an inaccessible path, and
/// [`io::ErrorKind::Unsupported`] while the native adapter is absent.
pub fn admit_office_path(path: &Path, kind: OfficeKind) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{kind} session refuses symbolic links"),
        ));
    }
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{kind} session requires a regular file"),
        ));
    }
    if metadata.len() > MAX_OFFICE_DOCUMENT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{kind} is {} bytes; native office sessions are limited to {MAX_OFFICE_DOCUMENT_BYTES} bytes",
                metadata.len()
            ),
        ));
    }

    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "{kind} needs the sandboxed LibreOfficeKit session adapter; it is not packaged, and the Fedora libreofficekit GTK bridge is not an allowed VCL fallback"
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::{admit_office_path, office_kind, OfficeKind, MAX_OFFICE_DOCUMENT_BYTES};
    use std::io;
    use std::path::Path;

    #[test]
    fn office_extensions_map_to_exact_families_case_insensitively() {
        assert_eq!(
            office_kind(Path::new("plan.ODT")),
            Some(OfficeKind::Document)
        );
        assert_eq!(
            office_kind(Path::new("budget.XlSx")),
            Some(OfficeKind::Spreadsheet)
        );
        assert_eq!(
            office_kind(Path::new("brief.PPTX")),
            Some(OfficeKind::Presentation)
        );
        assert_eq!(office_kind(Path::new("notes.md")), None);
        assert_eq!(office_kind(Path::new("report.docx.exe")), None);
    }

    #[test]
    fn oversized_office_input_is_rejected_before_adapter_admission() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("oversized.odt");
        let file = std::fs::File::create(&path).expect("create");
        file.set_len(MAX_OFFICE_DOCUMENT_BYTES + 1)
            .expect("sparse length");

        let error = admit_office_path(&path, OfficeKind::Document).expect_err("must reject");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("limited"));
    }

    #[cfg(unix)]
    #[test]
    fn office_symlink_is_rejected_without_following_its_target() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("target.xlsx");
        let link = dir.path().join("untrusted.xlsx");
        std::fs::write(&target, b"PK\x03\x04office-container").expect("write target");
        symlink(&target, &link).expect("create symlink");

        let error = admit_office_path(&link, OfficeKind::Spreadsheet).expect_err("must reject");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("refuses symbolic links"));
        assert_eq!(
            std::fs::read(&target).expect("read unchanged target"),
            b"PK\x03\x04office-container"
        );
    }

    #[test]
    fn available_file_never_claims_a_session_without_the_packaged_adapter() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("book.xlsx");
        std::fs::write(&path, b"PK\x03\x04hostile-office-container").expect("write");

        let error = admit_office_path(&path, OfficeKind::Spreadsheet).expect_err("must refuse");
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("not packaged"));
        assert!(error.to_string().contains("not an allowed VCL fallback"));
    }
}
