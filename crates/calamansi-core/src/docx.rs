//! Minimal Word (.docx) writing and reading.
//!
//! Writing produces a plain, valid document: one paragraph per line of text,
//! with page breaks between pages. Reading extracts paragraph text. Layout,
//! images and tables are out of scope; when Microsoft Word is installed,
//! PDF → Word uses Word itself for a faithful conversion instead.

use crate::error::{CoreError, Result};
use std::io::{Read, Seek, Write};
use zip::write::SimpleFileOptions;

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;

const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // Control characters other than tab are invalid in XML 1.0.
            c if (c as u32) < 0x20 && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// Write `pages` (each a block of text) as a .docx.
pub fn write<W: Write + Seek>(pages: &[String], out: W) -> Result<()> {
    let mut body = String::new();
    for (i, page) in pages.iter().enumerate() {
        if i > 0 {
            body.push_str(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#);
        }
        for line in page.replace("\r\n", "\n").split('\n') {
            if line.trim().is_empty() {
                body.push_str("<w:p/>");
            } else {
                body.push_str(&format!(
                    r#"<w:p><w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
                    escape(line)
                ));
            }
        }
    }
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    );
    let mut zip = zip::ZipWriter::new(out);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let zerr = |e: zip::result::ZipError| {
        CoreError::InvalidInput(format!("couldn't write the Word file: {e}"))
    };
    for (name, data) in [
        ("[Content_Types].xml", CONTENT_TYPES),
        ("_rels/.rels", RELS),
        ("word/document.xml", document.as_str()),
    ] {
        zip.start_file(name, opts).map_err(zerr)?;
        zip.write_all(data.as_bytes())?;
    }
    zip.finish().map_err(zerr)?;
    Ok(())
}

/// Paragraph text of a .docx, one paragraph per line.
pub fn read_text<R: Read + Seek>(input: R) -> Result<String> {
    let bad = |e: zip::result::ZipError| {
        CoreError::InvalidInput(format!("not a readable Word file: {e}"))
    };
    let mut zip = zip::ZipArchive::new(input).map_err(bad)?;
    let mut entry = zip.by_name("word/document.xml").map_err(bad)?;
    if entry.size() > 200 * 1024 * 1024 {
        return Err(CoreError::TooLarge("the document text is too large".into()));
    }
    let mut xml = String::new();
    entry.read_to_string(&mut xml)?;
    Ok(xml_to_text(&xml))
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Walk the tags: text inside `<w:t>`, tabs, breaks and paragraph ends.
fn xml_to_text(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    let mut in_text = false;
    while let Some(lt) = rest.find('<') {
        if in_text {
            out.push_str(&unescape(&rest[..lt]));
        }
        let Some(gt) = rest[lt..].find('>') else {
            break;
        };
        let tag = &rest[lt + 1..lt + gt];
        let name = tag
            .trim_start_matches('/')
            .split([' ', '/'])
            .next()
            .unwrap_or("");
        let closing = tag.starts_with('/');
        let self_closing = tag.ends_with('/');
        match name {
            "w:t" => in_text = !closing && !self_closing,
            "w:tab" if !closing => out.push('\t'),
            "w:br" | "w:cr" if !closing => out.push('\n'),
            "w:p" if closing || self_closing => out.push('\n'),
            _ => {}
        }
        rest = &rest[lt + gt + 1..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trip() {
        let mut buf = Cursor::new(Vec::new());
        write(
            &[
                "Hello <Calamansi> & co\n\nSecond\tline".into(),
                "Page two".into(),
            ],
            &mut buf,
        )
        .unwrap();
        let bytes = buf.into_inner();
        assert!(bytes.starts_with(b"PK"));
        let text = read_text(Cursor::new(bytes)).unwrap();
        assert!(text.contains("Hello <Calamansi> & co\n"));
        assert!(text.contains("Second\tline"));
        assert!(text.contains("Page two"));
    }

    #[test]
    fn rejects_non_zip() {
        assert!(read_text(Cursor::new(b"not a zip".to_vec())).is_err());
    }
}
