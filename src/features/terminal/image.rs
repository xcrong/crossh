//! Terminal image validation and Kitty image codecs.

use std::io::{Cursor, Read};

use flate2::read::ZlibDecoder;

use super::view::{KittyPlacement, MAX_DECODED_IMAGE_BYTES, MAX_IMAGE_DIMENSION};

pub(crate) fn terminal_image_format(bytes: &[u8]) -> Option<gpui::ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(gpui::ImageFormat::Png)
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(gpui::ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(gpui::ImageFormat::Gif)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(gpui::ImageFormat::Webp)
    } else {
        None
    }
}

pub(crate) fn terminal_image_within_limits(data: &[u8], format: gpui::ImageFormat) -> bool {
    if format != gpui::ImageFormat::Png {
        return true;
    }
    let Ok(reader) = png::Decoder::new(Cursor::new(data)).read_info() else {
        return false;
    };
    let width = reader.info().width as usize;
    let height = reader.info().height as usize;
    width > 0
        && height > 0
        && width <= MAX_IMAGE_DIMENSION
        && height <= MAX_IMAGE_DIMENSION
        && width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .is_some_and(|bytes| bytes <= MAX_DECODED_IMAGE_BYTES)
}

pub(crate) fn encode_rgba_png(pixels: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return None;
    }
    let expected = width.checked_mul(height)?.checked_mul(4)?;
    if expected != pixels.len() || pixels.len() > MAX_DECODED_IMAGE_BYTES {
        return None;
    }

    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(pixels).ok()?;
    }
    Some(encoded)
}

pub(crate) fn kitty_raw_to_png(
    data: &[u8],
    width: usize,
    height: usize,
    channels: usize,
) -> Option<Vec<u8>> {
    if !matches!(channels, 3 | 4) {
        return None;
    }
    let pixel_count = width.checked_mul(height)?;
    let expected = pixel_count.checked_mul(channels)?;
    if expected != data.len() {
        return None;
    }

    if channels == 4 {
        return encode_rgba_png(data, width, height);
    }

    let mut rgba = Vec::with_capacity(pixel_count.checked_mul(4)?);
    for rgb in data.chunks_exact(3) {
        rgba.extend_from_slice(rgb);
        rgba.push(0xff);
    }
    encode_rgba_png(&rgba, width, height)
}

pub(crate) fn crop_kitty_image(data: &[u8], placement: KittyPlacement) -> Option<Vec<u8>> {
    if placement.source_x.is_none()
        && placement.source_y.is_none()
        && placement.source_width.is_none()
        && placement.source_height.is_none()
    {
        return Some(data.to_vec());
    }
    if terminal_image_format(data) != Some(gpui::ImageFormat::Png) {
        return None;
    }

    let mut decoder = png::Decoder::new(Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let width = reader.info().width as usize;
    let height = reader.info().height as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let decoded_size = width.checked_mul(height)?.checked_mul(4)?;
    if decoded_size > MAX_DECODED_IMAGE_BYTES {
        return None;
    }
    let mut buffer = vec![0; reader.output_buffer_size()];
    let output = reader.next_frame(&mut buffer).ok()?;
    let bytes = &buffer[..output.buffer_size()];
    let rgba = match output.color_type {
        png::ColorType::Rgba if output.bit_depth == png::BitDepth::Eight => bytes.to_vec(),
        png::ColorType::Rgb if output.bit_depth == png::BitDepth::Eight => {
            let mut rgba = Vec::with_capacity(decoded_size);
            for pixel in bytes.chunks_exact(3) {
                rgba.extend_from_slice(pixel);
                rgba.push(0xff);
            }
            rgba
        }
        png::ColorType::Grayscale if output.bit_depth == png::BitDepth::Eight => {
            let mut rgba = Vec::with_capacity(decoded_size);
            for &gray in bytes {
                rgba.extend_from_slice(&[gray, gray, gray, 0xff]);
            }
            rgba
        }
        png::ColorType::GrayscaleAlpha if output.bit_depth == png::BitDepth::Eight => {
            let mut rgba = Vec::with_capacity(decoded_size);
            for pixel in bytes.chunks_exact(2) {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
            rgba
        }
        _ => return None,
    };

    let x = placement.source_x.unwrap_or(0).min(width);
    let y = placement.source_y.unwrap_or(0).min(height);
    let crop_width = placement
        .source_width
        .unwrap_or(width.saturating_sub(x))
        .min(width.saturating_sub(x));
    let crop_height = placement
        .source_height
        .unwrap_or(height.saturating_sub(y))
        .min(height.saturating_sub(y));
    if crop_width == 0 || crop_height == 0 {
        return None;
    }
    let mut cropped = Vec::with_capacity(crop_width.checked_mul(crop_height)?.checked_mul(4)?);
    for row in y..y + crop_height {
        let start = row.checked_mul(width)?.checked_add(x)?.checked_mul(4)?;
        let end = start.checked_add(crop_width.checked_mul(4)?)?;
        cropped.extend_from_slice(rgba.get(start..end)?);
    }
    encode_rgba_png(&cropped, crop_width, crop_height)
}

pub(crate) fn kitty_zlib_decode(data: &[u8]) -> Option<Vec<u8>> {
    let decoder = ZlibDecoder::new(data);
    let mut decoded = Vec::new();
    decoder
        .take((MAX_DECODED_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut decoded)
        .ok()?;
    (decoded.len() <= MAX_DECODED_IMAGE_BYTES).then_some(decoded)
}

pub(crate) fn kitty_parameter<'a>(control: &'a str, key: &str) -> Option<&'a str> {
    control.split(',').find_map(|field| {
        let (field_key, value) = field.split_once('=')?;
        (field_key == key).then_some(value)
    })
}

pub(crate) fn sanitize_kitty_notification_id(value: &str) -> Option<String> {
    let id = value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '+' | '.')
        })
        .take(128)
        .collect::<String>();
    (!id.is_empty()).then_some(id)
}

pub(crate) fn kitty_image_action<'a>(control: &'a str, stored_action: Option<&'a str>) -> &'a str {
    kitty_parameter(control, "a")
        .or(stored_action)
        .unwrap_or("t")
}
