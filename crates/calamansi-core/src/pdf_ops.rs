//! PDF creation, merging, splitting and text extraction.

use crate::error::{CoreError, Result};
use crate::image_ops;
use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream, StringFormat};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

/// Points per pixel when sizing a page to an image (96 DPI screen pixels).
const PT_PER_PX: f32 = 72.0 / 96.0;
const A4: (f32, f32) = (595.0, 842.0);

fn open(path: &Path) -> Result<Document> {
    image_ops::check_size(path)?;
    let doc = Document::load(path)?;
    if doc.is_encrypted() {
        return Err(CoreError::Pdf(
            "password-protected PDFs aren't supported yet".into(),
        ));
    }
    Ok(doc)
}

fn finish(
    mut doc: Document,
    pages_id: ObjectId,
    kids: Vec<Object>,
    out: &mut impl Write,
) -> Result<()> {
    let count = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => count,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.save_to(out)?;
    Ok(())
}

/// One page per image, each page sized to its image. Transparency is
/// flattened onto white and images are embedded as JPEG.
pub fn images_to_pdf(images: &[&Path], quality: u8, out: &mut impl Write) -> Result<()> {
    if images.is_empty() {
        return Err(CoreError::InvalidInput(
            "no images to put in the PDF".into(),
        ));
    }
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();
    for path in images {
        let img = image_ops::load(path)?;
        let (w, h) = (img.width(), img.height());
        let mut jpeg = Vec::new();
        image_ops::encode_jpeg(&img, quality, &mut jpeg)?;
        drop(img);

        let mut image_stream = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => w as i64,
                "Height" => h as i64,
                "ColorSpace" => "DeviceRGB",
                "BitsPerComponent" => 8,
                "Filter" => "DCTDecode",
            },
            jpeg,
        );
        image_stream.allows_compression = false;
        let image_id = doc.add_object(image_stream);

        let (pw, ph) = (w as f32 * PT_PER_PX, h as f32 * PT_PER_PX);
        let content = Content {
            operations: vec![
                Operation::new("q", vec![]),
                Operation::new(
                    "cm",
                    vec![pw.into(), 0.into(), 0.into(), ph.into(), 0.into(), 0.into()],
                ),
                Operation::new("Do", vec![Object::Name(b"Im0".to_vec())]),
                Operation::new("Q", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode()?));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), pw.into(), ph.into()],
            "Contents" => content_id,
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image_id } },
        });
        kids.push(page_id.into());
    }
    finish(doc, pages_id, kids, out)
}

/// Plain text to an A4 PDF in Courier. Characters outside Latin-1 are
/// replaced with `?` (the standard PDF fonts can't show them).
pub fn text_to_pdf(text: &str, out: &mut impl Write) -> Result<()> {
    const SIZE: f32 = 10.0;
    const LEADING: f32 = 12.0;
    const MARGIN: f32 = 50.0;
    let cols = ((A4.0 - 2.0 * MARGIN) / (SIZE * 0.6)) as usize;
    let rows = ((A4.1 - 2.0 * MARGIN) / LEADING) as usize;

    let mut lines: Vec<Vec<u8>> = Vec::new();
    for raw in text.replace("\r\n", "\n").split('\n') {
        let bytes: Vec<u8> = raw
            .replace('\t', "    ")
            .chars()
            .map(|c| {
                if (c as u32) < 256 {
                    c as u32 as u8
                } else {
                    b'?'
                }
            })
            .collect();
        if bytes.is_empty() {
            lines.push(Vec::new());
        }
        for chunk in bytes.chunks(cols.max(1)) {
            lines.push(chunk.to_vec());
        }
    }

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Courier",
        "Encoding" => "WinAnsiEncoding",
    });
    let mut kids = Vec::new();
    let pages: Vec<&[Vec<u8>]> = if lines.is_empty() {
        vec![&[][..]]
    } else {
        lines.chunks(rows).collect()
    };
    for page_lines in pages {
        let mut ops = vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec![Object::Name(b"F1".to_vec()), SIZE.into()]),
            Operation::new("TL", vec![LEADING.into()]),
            Operation::new("Td", vec![MARGIN.into(), (A4.1 - MARGIN - SIZE).into()]),
        ];
        for line in page_lines {
            ops.push(Operation::new(
                "Tj",
                vec![Object::String(line.clone(), StringFormat::Literal)],
            ));
            ops.push(Operation::new("T*", vec![]));
        }
        ops.push(Operation::new("ET", vec![]));
        let content_id = doc.add_object(Stream::new(
            dictionary! {},
            Content { operations: ops }.encode()?,
        ));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), A4.0.into(), A4.1.into()],
            "Contents" => content_id,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        });
        kids.push(page_id.into());
    }
    finish(doc, pages_id, kids, out)
}

/// Page attributes a page may inherit from its ancestors in the page tree.
const INHERITABLE: &[&[u8]] = &[b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// Copy inherited attributes onto the page itself, so it survives being
/// moved into a new page tree.
fn resolved_page(doc: &Document, page_id: ObjectId) -> Result<Dictionary> {
    let mut page = doc.get_dictionary(page_id)?.clone();
    let mut parent = page.get(b"Parent").and_then(Object::as_reference).ok();
    let mut depth = 0;
    while let Some(pid) = parent {
        depth += 1;
        if depth > 64 {
            break; // malformed tree; avoid looping forever
        }
        let Ok(node) = doc.get_dictionary(pid) else {
            break;
        };
        for key in INHERITABLE {
            if !page.has(key) {
                if let Ok(v) = node.get(key) {
                    page.set(key.to_vec(), v.clone());
                }
            }
        }
        parent = node.get(b"Parent").and_then(Object::as_reference).ok();
    }
    Ok(page)
}

/// Merge PDFs in the given order.
pub fn merge(inputs: &[&Path], out: &mut impl Write) -> Result<()> {
    if inputs.len() < 2 {
        return Err(CoreError::InvalidInput(
            "choose at least two PDFs to merge".into(),
        ));
    }
    let mut next_id = 1;
    let mut objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut page_ids: Vec<ObjectId> = Vec::new();
    for path in inputs {
        let mut doc = open(path)?;
        doc.renumber_objects_with(next_id);
        next_id = doc.max_id + 1;
        let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
        let mut resolved = Vec::with_capacity(pages.len());
        for id in &pages {
            resolved.push((*id, resolved_page(&doc, *id)?));
        }
        for (id, obj) in doc.objects {
            let skip = matches!(
                obj.type_name(),
                Ok(b"Catalog") | Ok(b"Pages") | Ok(b"Page") | Ok(b"Outlines") | Ok(b"Outline")
            );
            if !skip {
                objects.insert(id, obj);
            }
        }
        for (id, dict) in resolved {
            objects.insert(id, Object::Dictionary(dict));
            page_ids.push(id);
        }
    }

    let mut doc = Document::with_version("1.5");
    doc.objects = objects;
    doc.max_id = next_id;
    let pages_id = doc.new_object_id();
    for id in &page_ids {
        if let Some(Object::Dictionary(d)) = doc.objects.get_mut(id) {
            d.set("Parent", pages_id);
            d.remove(b"Annots"); // links into the old documents would dangle
        }
    }
    let kids = page_ids.into_iter().map(Object::from).collect();
    finish(doc, pages_id, kids, out)
}

pub fn page_count(path: &Path) -> Result<usize> {
    Ok(open(path)?.get_pages().len())
}

/// Write page `n` (1-based) of `path` as its own PDF.
pub fn extract_page(path: &Path, n: u32, out: &mut impl Write) -> Result<()> {
    let mut doc = open(path)?;
    let total = doc.get_pages().len() as u32;
    if n == 0 || n > total {
        return Err(CoreError::InvalidInput(format!(
            "page {n} doesn't exist (the PDF has {total})"
        )));
    }
    let others: Vec<u32> = (1..=total).filter(|&p| p != n).collect();
    doc.delete_pages(&others);
    doc.prune_objects();
    doc.save_to(out)?;
    Ok(())
}

/// Text of each page, best effort. Scanned pages come back empty.
pub fn page_texts(path: &Path) -> Result<Vec<String>> {
    let doc = open(path)?;
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    Ok(pages
        .into_iter()
        .map(|p| {
            doc.extract_text(&[p])
                .map(|t| t.trim_end().to_string())
                .unwrap_or_default()
        })
        .collect())
}

/// Best-effort text extraction. Scanned PDFs (images only) produce no text.
pub fn extract_text(path: &Path) -> Result<String> {
    let doc = open(path)?;
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    let mut text = String::new();
    for p in pages {
        if let Ok(t) = doc.extract_text(&[p]) {
            text.push_str(t.trim_end());
            text.push_str("\n\n");
        }
    }
    Ok(text)
}

/// Longest side of a rendered page, in pixels.
pub const RENDER_MAX_SIDE: f32 = 4000.0;

/// Render every page to an RGBA image at `dpi` (capped at
/// [`RENDER_MAX_SIDE`]), calling `each(page_number, image)` in order.
pub fn render_pages(
    path: &Path,
    dpi: f32,
    cancel: &std::sync::atomic::AtomicBool,
    mut each: impl FnMut(usize, image::DynamicImage) -> Result<()>,
) -> Result<()> {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::hayro_syntax::Pdf;
    use hayro::vello_cpu::color::palette::css::WHITE;
    use hayro::{render, PixmapSettings, RenderCache, RenderSettings};

    image_ops::check_size(path)?;
    let data = std::fs::read(path)?;
    let pdf =
        Pdf::new(data).map_err(|e| CoreError::Pdf(format!("couldn't read this PDF ({e:?})")))?;
    let cache = RenderCache::new();
    let interpreter = InterpreterSettings::default();
    let pages = pdf.pages();
    if pages.is_empty() {
        return Err(CoreError::Pdf("the PDF has no pages".into()));
    }
    for (i, page) in pages.iter().enumerate() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        let (w, h) = page.render_dimensions();
        let mut scale = dpi / 72.0;
        let longest = w.max(h) * scale;
        if longest > RENDER_MAX_SIDE {
            scale *= RENDER_MAX_SIDE / longest;
        }
        let settings = PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
        };
        let pixmap = render(
            page,
            &cache,
            &interpreter,
            &RenderSettings::default(),
            &settings,
        );
        let png = pixmap
            .into_png()
            .map_err(|e| CoreError::Pdf(format!("couldn't render page {} ({e:?})", i + 1)))?;
        let img = image::load_from_memory(&png)?;
        each(i + 1, img)?;
    }
    Ok(())
}

/// Rotate every page clockwise by `degrees` (a multiple of 90).
pub fn rotate_pages(path: &Path, degrees: i64, out: &mut impl Write) -> Result<()> {
    let mut doc = open(path)?;
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    for id in pages {
        let current = resolved_page(&doc, id)?
            .get(b"Rotate")
            .ok()
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        if let Ok(Object::Dictionary(d)) = doc.get_object_mut(id) {
            d.set("Rotate", (current + degrees).rem_euclid(360));
        }
    }
    doc.save_to(out)?;
    Ok(())
}

/// Write the pages in reverse order.
pub fn reverse_pages(path: &Path, out: &mut impl Write) -> Result<()> {
    let doc = open(path)?;
    let mut page_ids: Vec<ObjectId> = doc.get_pages().into_values().collect();
    page_ids.reverse();
    let mut resolved = Vec::new();
    for id in &page_ids {
        resolved.push((*id, resolved_page(&doc, *id)?));
    }
    let mut doc = doc;
    let pages_id = doc.new_object_id();
    for (id, mut dict) in resolved {
        dict.set("Parent", pages_id);
        doc.objects.insert(id, Object::Dictionary(dict));
    }
    let kids = page_ids.into_iter().map(Object::from).collect();
    finish(doc, pages_id, kids, out)
}

/// Re-save with compressed streams and unused objects removed.
pub fn compress(path: &Path, out: &mut impl Write) -> Result<()> {
    let mut doc = open(path)?;
    doc.prune_objects();
    doc.delete_zero_length_streams();
    doc.compress();
    doc.save_to(out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgba, RgbaImage};
    use std::fs::File;
    use std::io::BufWriter;

    fn write_png(path: &Path, w: u32, h: u32) {
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(w, h, Rgba([200, 30, 30, 255])))
            .save(path)
            .unwrap();
    }

    fn write(path: &Path, f: impl FnOnce(&mut BufWriter<File>) -> Result<()>) {
        let mut w = BufWriter::new(File::create(path).unwrap());
        f(&mut w).unwrap();
        w.flush().unwrap();
    }

    #[test]
    fn images_merge_split_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let (a, b) = (d.join("a.png"), d.join("b.png"));
        write_png(&a, 40, 30);
        write_png(&b, 20, 50);

        let one = d.join("one.pdf");
        write(&one, |w| images_to_pdf(&[&a, &b], 85, w));
        assert_eq!(page_count(&one).unwrap(), 2);

        let text = d.join("text.pdf");
        let long = "line\n".repeat(150);
        write(&text, |w| {
            text_to_pdf(&format!("Hello Calamansi\n{long}"), w)
        });
        assert_eq!(page_count(&text).unwrap(), 3);
        assert!(extract_text(&text).unwrap().contains("Hello Calamansi"));

        let merged = d.join("merged.pdf");
        write(&merged, |w| merge(&[&one, &text], w));
        assert_eq!(page_count(&merged).unwrap(), 5);
        assert!(extract_text(&merged).unwrap().contains("Hello Calamansi"));

        let rotated = d.join("rot.pdf");
        write(&rotated, |w| rotate_pages(&merged, 90, w));
        let doc = Document::load(&rotated).unwrap();
        let first = *doc.get_pages().values().next().unwrap();
        assert_eq!(
            resolved_page(&doc, first)
                .unwrap()
                .get(b"Rotate")
                .unwrap()
                .as_i64()
                .unwrap(),
            90
        );

        let reversed = d.join("rev.pdf");
        write(&reversed, |w| reverse_pages(&merged, w));
        assert_eq!(page_count(&reversed).unwrap(), 5);
        let rev = Document::load(&reversed).unwrap();
        let first = *rev.get_pages().values().next().unwrap();
        assert!(
            rev.extract_text(&[1]).unwrap().contains("line"),
            "text page now first"
        );
        let _ = first;

        let small = d.join("small.pdf");
        write(&small, |w| compress(&merged, w));
        assert_eq!(page_count(&small).unwrap(), 5);

        let mut rendered = Vec::new();
        render_pages(
            &text,
            72.0,
            &std::sync::atomic::AtomicBool::new(false),
            |n, img| {
                rendered.push((n, img.width(), img.height()));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(rendered.len(), 3);
        assert_eq!((rendered[0].1, rendered[0].2), (595, 842));

        let page3 = d.join("p3.pdf");
        write(&page3, |w| extract_page(&merged, 3, w));
        assert_eq!(page_count(&page3).unwrap(), 1);
        assert!(extract_text(&page3).unwrap().contains("Hello Calamansi"));
    }

    #[test]
    fn merge_needs_two() {
        let mut out = Vec::new();
        assert!(merge(&[Path::new("x.pdf")], &mut out).is_err());
    }

    #[test]
    fn non_latin_text_is_replaced_not_rejected() {
        let mut out = Vec::new();
        text_to_pdf("नमस्ते and café", &mut out).unwrap();
        assert!(out.starts_with(b"%PDF-1.5"));
    }
}
