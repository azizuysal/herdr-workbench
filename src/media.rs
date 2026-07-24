//! Bounded decoding for visual file previews.

use std::{
    fs,
    io::{Cursor, Read},
    path::Path,
    sync::Arc,
};

use hayro::{
    RenderCache, RenderSettings, hayro_interpret::InterpreterSettings, hayro_syntax::Pdf, render,
    vello_cpu::color::palette::css::WHITE,
};
use image::{ImageFormat, ImageReader, Limits, RgbaImage};

const MAX_ENCODED_BYTES: u64 = 32 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 16_384;
const MAX_IMAGE_ALLOCATION: u64 = 128 * 1024 * 1024;
const RETAINED_IMAGE_DIMENSION: u32 = 2_400;
const PDF_RENDER_WIDTH: u16 = 1_600;
const PDF_RENDER_HEIGHT: u16 = 1_800;

#[derive(Clone, Debug, PartialEq)]
pub struct VisualPreview {
    pub pixels: RgbaImage,
    pub source_width: u32,
    pub source_height: u32,
    pub page_count: Option<usize>,
    pub page_index: Option<usize>,
    pdf_bytes: Option<Arc<Vec<u8>>>,
}

impl VisualPreview {
    pub fn raster(pixels: RgbaImage, source_width: u32, source_height: u32) -> Self {
        Self {
            pixels,
            source_width,
            source_height,
            page_count: None,
            page_index: None,
            pdf_bytes: None,
        }
    }

    pub fn show_pdf_page(&mut self, page_index: usize) -> Result<(), String> {
        let bytes = self
            .pdf_bytes
            .as_ref()
            .ok_or_else(|| "this visual preview is not a PDF".to_string())?;
        let page_count = self
            .page_count
            .ok_or_else(|| "PDF page count is unavailable".to_string())?;
        if page_index >= page_count {
            return Err(format!(
                "PDF page {} is outside the 1–{page_count} range",
                page_index + 1
            ));
        }
        let rendered = render_pdf_page(Arc::clone(bytes), page_index)?;
        self.pixels = rendered.pixels;
        self.source_width = rendered.source_width;
        self.source_height = rendered.source_height;
        self.page_index = Some(page_index);
        Ok(())
    }
}

pub fn load(path: &Path) -> Result<Option<VisualPreview>, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    let mut file =
        fs::File::open(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut signature = [0_u8; 16];
    let signature_length = file
        .read(&mut signature)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let signature = &signature[..signature_length];
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("");
    let is_pdf = signature.starts_with(b"%PDF-") || extension.eq_ignore_ascii_case("pdf");
    let image_format = image::guess_format(signature)
        .ok()
        .or_else(|| ImageFormat::from_extension(extension));

    if !is_pdf && image_format.is_none() {
        return Ok(None);
    }
    if metadata.len() > MAX_ENCODED_BYTES {
        return Err(format!(
            "{} is {} MiB; visual previews are limited to {} MiB",
            path.display(),
            metadata.len().div_ceil(1024 * 1024),
            MAX_ENCODED_BYTES / 1024 / 1024
        ));
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len().min(MAX_ENCODED_BYTES)).unwrap_or_default(),
    );
    fs::File::open(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?
        .take(MAX_ENCODED_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_ENCODED_BYTES {
        return Err(format!(
            "{} changed while opening and now exceeds the {} MiB visual-preview limit",
            path.display(),
            MAX_ENCODED_BYTES / 1024 / 1024
        ));
    }
    if is_pdf {
        render_pdf(path, bytes).map(Some)
    } else {
        decode_image(path, bytes, image_format.expect("checked image format")).map(Some)
    }
}

fn decode_image(path: &Path, bytes: Vec<u8>, format: ImageFormat) -> Result<VisualPreview, String> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_ALLOCATION);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|error| format!("cannot decode image {}: {error}", path.display()))?;
    let source_width = decoded.width();
    let source_height = decoded.height();
    let pixels = decoded.thumbnail(RETAINED_IMAGE_DIMENSION, RETAINED_IMAGE_DIMENSION);
    Ok(VisualPreview::raster(
        pixels.into_rgba8(),
        source_width,
        source_height,
    ))
}

fn render_pdf(path: &Path, bytes: Vec<u8>) -> Result<VisualPreview, String> {
    let bytes = Arc::new(bytes);
    let pdf = Pdf::new(Arc::clone(&bytes))
        .map_err(|error| format!("cannot parse PDF {}: {error:?}", path.display()))?;
    let page_count = pdf.pages().len();
    if page_count == 0 {
        return Err(format!("PDF {} has no pages", path.display()));
    }
    let mut preview = render_pdf_page(Arc::clone(&bytes), 0)?;
    preview.page_count = Some(page_count);
    preview.page_index = Some(0);
    preview.pdf_bytes = Some(bytes);
    Ok(preview)
}

fn render_pdf_page(bytes: Arc<Vec<u8>>, page_index: usize) -> Result<VisualPreview, String> {
    let pdf = Pdf::new(bytes)
        .map_err(|error| format!("cannot parse PDF while changing pages: {error:?}"))?;
    let page = pdf
        .pages()
        .get(page_index)
        .ok_or_else(|| format!("PDF page {} is unavailable", page_index + 1))?;
    let (source_width, source_height) = page.render_dimensions();
    if !source_width.is_finite()
        || !source_height.is_finite()
        || source_width <= 0.0
        || source_height <= 0.0
    {
        return Err(format!(
            "PDF page {} has invalid dimensions",
            page_index + 1
        ));
    }
    let scale = (f32::from(PDF_RENDER_WIDTH) / source_width)
        .min(f32::from(PDF_RENDER_HEIGHT) / source_height)
        .min(2.0);
    let width = (source_width * scale)
        .round()
        .clamp(1.0, f32::from(PDF_RENDER_WIDTH)) as u16;
    let height = (source_height * scale)
        .round()
        .clamp(1.0, f32::from(PDF_RENDER_HEIGHT)) as u16;
    let pixmap = render(
        page,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &RenderSettings {
            x_scale: scale,
            y_scale: scale,
            width: Some(width),
            height: Some(height),
            bg_color: WHITE,
        },
    );
    let png = pixmap
        .into_png()
        .map_err(|error| format!("cannot encode PDF page {}: {error}", page_index + 1))?;
    let pixels = image::load_from_memory_with_format(&png, ImageFormat::Png)
        .map_err(|error| format!("cannot decode PDF page {}: {error}", page_index + 1))?
        .into_rgba8();
    Ok(VisualPreview {
        pixels,
        source_width: source_width.round() as u32,
        source_height: source_height.round() as u32,
        page_count: None,
        page_index: Some(page_index),
        pdf_bytes: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};

    #[test]
    fn decodes_png_and_preserves_dimensions() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("sample.png");
        DynamicImage::ImageRgba8(ImageBuffer::from_pixel(3, 2, Rgba([10, 20, 30, 255])))
            .save_with_format(&path, ImageFormat::Png)
            .expect("fixture");

        let preview = load(&path).expect("decode").expect("visual");

        assert_eq!((preview.source_width, preview.source_height), (3, 2));
        assert_eq!(preview.pixels.get_pixel(0, 0).0, [10, 20, 30, 255]);
        assert_eq!(preview.page_count, None);
        assert_eq!(preview.page_index, None);
    }

    #[test]
    fn decodes_every_documented_raster_format() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let image =
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(2, 2, Rgba([40, 80, 120, 255])));
        for (extension, format) in [
            ("png", ImageFormat::Png),
            ("jpg", ImageFormat::Jpeg),
            ("gif", ImageFormat::Gif),
            ("webp", ImageFormat::WebP),
            ("bmp", ImageFormat::Bmp),
            ("ico", ImageFormat::Ico),
        ] {
            let path = directory.path().join(format!("sample.{extension}"));
            image
                .save_with_format(&path, format)
                .unwrap_or_else(|error| panic!("cannot create {extension} fixture: {error}"));

            let preview = load(&path)
                .unwrap_or_else(|error| panic!("cannot decode {extension}: {error}"))
                .expect("visual");

            assert_eq!(
                (preview.source_width, preview.source_height),
                (2, 2),
                "{extension}"
            );
        }
    }

    #[test]
    fn ordinary_binary_is_not_misclassified_as_visual() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("data.bin");
        fs::write(&path, [0, 1, 2, 3]).expect("fixture");

        assert!(load(&path).expect("classification").is_none());
    }

    #[test]
    fn invalid_known_image_reports_an_actionable_error() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("broken.png");
        fs::write(&path, b"not a png").expect("fixture");

        let error = load(&path).expect_err("invalid image");

        assert!(error.contains("cannot decode image"));
        assert!(error.contains("broken.png"));
    }

    #[test]
    fn rejects_oversized_visual_input_before_decoding() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("oversized.png");
        let file = fs::File::create(&path).expect("fixture");
        file.set_len(MAX_ENCODED_BYTES + 1).expect("sparse fixture");

        let error = load(&path).expect_err("oversized image");

        assert!(error.contains("visual previews are limited to 32 MiB"));
    }

    #[test]
    fn renders_the_first_pdf_page_without_an_external_tool() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("sample.pdf");
        fs::write(&path, pdf_with_pages(1)).expect("fixture");

        let preview = load(&path).expect("render").expect("visual");

        assert_eq!(preview.page_count, Some(1));
        assert_eq!(preview.page_index, Some(0));
        assert_eq!((preview.source_width, preview.source_height), (100, 100));
        assert!(!preview.pixels.is_empty());
    }

    #[test]
    fn navigates_every_pdf_page_without_retaining_all_rasters() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("multipage.pdf");
        fs::write(&path, pdf_with_pages(2)).expect("fixture");
        let mut preview = load(&path).expect("render").expect("visual");

        preview.show_pdf_page(1).expect("second page");

        assert_eq!(preview.page_count, Some(2));
        assert_eq!(preview.page_index, Some(1));
        assert!(preview.show_pdf_page(2).is_err());
    }

    fn pdf_with_pages(page_count: usize) -> Vec<u8> {
        let content_object = page_count + 3;
        let kids = (0..page_count)
            .map(|index| format!("{} 0 R", index + 3))
            .collect::<Vec<_>>()
            .join(" ");
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            format!("<< /Type /Pages /Kids [{kids}] /Count {page_count} >>"),
        ];
        for _ in 0..page_count {
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents {content_object} 0 R >>"
            ));
        }
        objects.push("<< /Length 25 >>\nstream\n0 0 1 rg 0 0 100 100 re f\nendstream".to_string());
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref_offset = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        pdf.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        pdf
    }
}
