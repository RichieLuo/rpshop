//! Local bitmap transformations with bounded decoding and atomic batch edits.

use std::{
    collections::BTreeSet,
    io::{self, Cursor, Seek, SeekFrom, Write},
};

use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use image::{DynamicImage, ImageFormat, ImageReader, Limits};

use crate::model::{Project, WidgetKind};

const MAX_BYTES: usize = 12 * 1024 * 1024;
const MAX_DIMENSION: u32 = 8192;
const MAX_PIXELS: u64 = 32_000_000;

#[derive(Default)]
struct PngOutput(Cursor<Vec<u8>>);

impl Write for PngOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .0
            .position()
            .checked_add(bytes.len() as u64)
            .is_none_or(|length| length > MAX_BYTES as u64)
        {
            return Err(io::Error::other("翻转后的 PNG 图片超过 12 MB 限制"));
        }
        self.0.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Seek for PngOutput {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let position = self.0.seek(position)?;
        if position > MAX_BYTES as u64 {
            return Err(io::Error::other("翻转后的 PNG 图片超过 12 MB 限制"));
        }
        Ok(position)
    }
}

fn decode(data: &str) -> Result<DynamicImage> {
    ensure!(data.len() <= MAX_BYTES * 4 / 3 + 64, "图片超过 12 MB 限制");
    let (header, payload) = data.split_once(',').context("图片必须为 base64 data URI")?;
    let format = match header {
        "data:image/png;base64" => ImageFormat::Png,
        "data:image/jpeg;base64" => ImageFormat::Jpeg,
        "data:image/gif;base64" => ImageFormat::Gif,
        "data:image/webp;base64" => ImageFormat::WebP,
        _ => bail!("只支持 PNG、JPEG、GIF 和 WebP 图片"),
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .context("图片 base64 无效")?;
    ensure!(bytes.len() <= MAX_BYTES, "图片超过 12 MB 限制");
    ensure!(
        image::guess_format(&bytes).context("无法识别图片格式")? == format,
        "图片内容与 MIME 类型不匹配"
    );
    let (width, height) = ImageReader::with_format(Cursor::new(&bytes), format)
        .into_dimensions()
        .context("无法读取图片尺寸")?;
    ensure!(
        width > 0
            && height > 0
            && width <= MAX_DIMENSION
            && height <= MAX_DIMENSION
            && u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "图片尺寸不能超过 8192 px 或 3200 万像素"
    );
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    // Allows the maximum permitted 16-bit RGBA image plus decoder buffers.
    limits.max_alloc = Some(256 * 1024 * 1024);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    reader.decode().context("无法解码图片")
}

/// Mirror pixels locally and return a PNG data URI. The PNG output preserves
/// alpha, including the color channels of fully transparent pixels.
pub fn flip_data_uri(data: &str, horizontal: bool) -> Result<String> {
    let image = decode(data)?;
    let flipped = if horizontal {
        image.fliph()
    } else {
        image.flipv()
    };
    let mut output = PngOutput::default();
    flipped
        .write_to(&mut output, ImageFormat::Png)
        .context("无法编码翻转后的 PNG 图片")?;
    let bytes = output.0.into_inner();
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// Flip selected unlocked image widgets that have an image source. All images
/// are transformed before committing, so one invalid image leaves the entire
/// project unchanged. Geometry, groups, text and flags are retained verbatim.
pub fn flip_images(
    project: &mut Project,
    page: usize,
    ids: &BTreeSet<u64>,
    horizontal: bool,
) -> Result<usize> {
    let page_data = project.pages.get(page).context("图片所在页面不存在")?;
    let mut replacements = Vec::new();
    for (position, widget) in page_data.widgets.iter().enumerate() {
        if widget.locked || widget.kind != WidgetKind::Image || !ids.contains(&widget.id) {
            continue;
        }
        if let Some(data) = &widget.image_data {
            let flipped = flip_data_uri(data, horizontal)
                .with_context(|| format!("翻转图片「{}」失败", widget.name))?;
            replacements.push((position, flipped));
        }
    }
    let count = replacements.len();
    for (position, data) in replacements {
        project.pages[page].widgets[position].image_data = Some(data);
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Widget;
    use image::{Rgba, RgbaImage};

    fn pixels() -> RgbaImage {
        RgbaImage::from_fn(2, 3, |x, y| {
            Rgba(
                [
                    [255, 0, 0, 255],
                    [0, 255, 0, 128],
                    [0, 0, 255, 0],
                    [255, 255, 0, 64],
                    [255, 0, 255, 200],
                    [0, 255, 255, 255],
                ][(y * 2 + x) as usize],
            )
        })
    }

    fn png_data() -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(pixels())
            .write_to(&mut output, ImageFormat::Png)
            .unwrap();
        output.into_inner()
    }

    fn uri(bytes: &[u8]) -> String {
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
            }
        }
        !crc
    }

    #[test]
    fn horizontal_and_vertical_flips_preserve_every_rgba_channel() {
        let original = pixels();
        for horizontal in [true, false] {
            let flipped = flip_data_uri(&uri(&png_data()), horizontal).unwrap();
            assert!(flipped.starts_with("data:image/png;base64,"));
            let image = decode(&flipped).unwrap().into_rgba8();
            assert_eq!(image.dimensions(), (2, 3));
            for (x, y, pixel) in image.enumerate_pixels() {
                assert_eq!(
                    *pixel,
                    *original.get_pixel(
                        if horizontal { 1 - x } else { x },
                        if horizontal { y } else { 2 - y }
                    )
                );
            }
            let restored = flip_data_uri(&flipped, horizontal).unwrap();
            assert_eq!(decode(&restored).unwrap().into_rgba8(), original);
        }
    }

    #[test]
    fn image_errors_leave_the_entire_batch_unchanged() {
        let mut project = Project::blank();
        let first_id = project.allocate_id();
        let mut first = Widget::new(first_id, WidgetKind::Image, 15.0, 20.0);
        first.image_data = Some(uri(&png_data()));
        first.rotation = 37.0;
        first.text = "保留文本".into();
        let group = project.allocate_id();
        first.groups.push(group);
        let second_id = project.allocate_id();
        let mut second = Widget::new(second_id, WidgetKind::Image, 40.0, 80.0);
        second.image_data = Some("data:image/png;base64,broken!".into());
        project.pages[0].widgets.extend([first, second]);
        let original = project.clone();
        assert!(
            flip_images(
                &mut project,
                0,
                &BTreeSet::from([first_id, second_id]),
                true
            )
            .is_err()
        );
        assert_eq!(project, original);
        assert!(flip_images(&mut project, 99, &BTreeSet::from([first_id]), true).is_err());
        assert_eq!(project, original);
        assert_eq!(
            flip_images(&mut project, 0, &BTreeSet::from([first_id]), true).unwrap(),
            1
        );
        let mut expected = original;
        expected.pages[0].widgets[0].image_data = Some(
            flip_data_uri(
                expected.pages[0].widgets[0].image_data.as_ref().unwrap(),
                true,
            )
            .unwrap(),
        );
        assert_eq!(project, expected);
    }

    #[test]
    fn non_images_missing_sources_and_locked_widgets_are_skipped() {
        let mut project = Project::blank();
        let mut ids = BTreeSet::new();
        for (kind, locked, data) in [
            (WidgetKind::Rectangle, false, Some(uri(&png_data()))),
            (WidgetKind::Image, false, None),
            (WidgetKind::Image, true, Some(uri(&png_data()))),
        ] {
            let id = project.allocate_id();
            ids.insert(id);
            let mut widget = Widget::new(id, kind, 0.0, 0.0);
            widget.locked = locked;
            widget.image_data = data;
            project.pages[0].widgets.push(widget);
        }
        let original = project.clone();
        assert_eq!(flip_images(&mut project, 0, &ids, true).unwrap(), 0);
        assert_eq!(project, original);
    }

    #[test]
    fn rejects_bad_mime_base64_oversized_inputs_and_excessive_dimensions() {
        let png = uri(&png_data());
        assert!(flip_data_uri(&png.replace("image/png", "image/jpeg"), true).is_err());
        assert!(flip_data_uri("data:image/svg+xml;base64,AAAA", true).is_err());
        assert!(flip_data_uri("data:image/png;base64,!!!!", true).is_err());
        let oversized = format!(
            "data:image/png;base64,{}",
            "A".repeat(MAX_BYTES * 4 / 3 + 65)
        );
        assert!(
            flip_data_uri(&oversized, true)
                .unwrap_err()
                .to_string()
                .contains("12 MB")
        );
        for (width, height) in [(8193u32, 1u32), (8000u32, 5000u32)] {
            let mut bytes = png_data();
            bytes[16..20].copy_from_slice(&width.to_be_bytes());
            bytes[20..24].copy_from_slice(&height.to_be_bytes());
            let crc = crc32(&bytes[12..29]);
            bytes[29..33].copy_from_slice(&crc.to_be_bytes());
            assert!(
                flip_data_uri(&uri(&bytes), true)
                    .unwrap_err()
                    .to_string()
                    .contains("图片尺寸")
            );
        }
        let mut output = PngOutput::default();
        output.seek(SeekFrom::Start(MAX_BYTES as u64)).unwrap();
        assert!(output.write_all(&[1]).is_err());
        assert!(output.0.into_inner().is_empty());
    }
}
