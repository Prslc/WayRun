use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom};

use tiny_skia::{IntSize, Pixmap, PixmapRef, PremultipliedColorU8};

const PNG_MAGIC: [u8; 8] = *b"\x89PNG\r\n\x1a\n";
const JPEG_MAGIC: [u8; 2] = [0xff, 0xd8];

/// Contain-fit `path` into a `size`×`size` box. PNG and JPEG stream from the
/// file and decode at the target scale; the other formats read it whole.
pub(super) fn decode(path: &str, size: u32) -> Option<Pixmap> {
    let source = streamed_source(path, size).or_else(|| {
        let data = fs::read(path).ok()?;
        Pixmap::decode_png(&data)
            .ok()
            .or_else(|| decode_image(&data))
    })?;
    let mut target = Pixmap::new(size, size)?;
    resample(source.as_ref(), &mut target);
    Some(target)
}

/// The image's intrinsic size from its header, which a preview pane fits to.
pub(super) fn dimensions(path: &str) -> Option<(u32, u32)> {
    image::image_dimensions(path).ok()
}

/// Read the magic, then hand the file back at its start to a decoder that
/// scales while it reads. A short/unseekable file and an interlaced PNG decline.
fn streamed_source(path: &str, size: u32) -> Option<Pixmap> {
    let mut file = File::open(path).ok()?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic).ok()?;
    file.seek(SeekFrom::Start(0)).ok()?;
    let reader = BufReader::new(file);
    if magic == PNG_MAGIC {
        decode_png(reader, size)
    } else if magic[..2] == JPEG_MAGIC {
        decode_jpeg(reader, size)
    } else {
        None
    }
}

/// Decode a PNG row by row, box-averaging straight into the preview box, so a
/// 4K source never materializes its full RGBA buffer. An interlaced PNG declines.
fn decode_png<R: Read>(reader: R, size: u32) -> Option<Pixmap> {
    let mut decoder = png::Decoder::new(reader);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let (source_w, source_h, interlaced) = {
        let info = reader.info();
        (info.width, info.height, info.interlaced)
    };
    if interlaced || source_w == 0 || source_h == 0 {
        return None;
    }
    let channels = match reader.output_color_type().0 {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return None,
    };
    let scale = (size as f32 / source_w as f32)
        .min(size as f32 / source_h as f32)
        .min(1.0);
    let out_w = ((source_w as f32 * scale).round() as u32).max(1);
    let out_h = ((source_h as f32 * scale).round() as u32).max(1);
    let mut out = Pixmap::new(out_w, out_h)?;

    let mut columns = vec![0u32; out_w as usize];
    for x in 0..source_w {
        columns[source_column(x, source_w, out_w)] += 1;
    }
    let mut sums = vec![[0u64; 4]; out_w as usize];
    let mut current: Option<(u32, u64)> = None;
    for y in 0..source_h {
        let row = reader.next_row().ok()??;
        let out_y = (u64::from(y) * u64::from(out_h) / u64::from(source_h)) as u32;
        current = match current {
            Some((row, rows)) if row == out_y => Some((out_y, rows + 1)),
            Some((row, rows)) => {
                flush_row(&mut out, row, &sums, &columns, rows);
                sums.fill([0; 4]);
                Some((out_y, 1))
            }
            None => Some((out_y, 1)),
        };
        let bytes = row.data();
        for x in 0..source_w as usize {
            let pixel = &bytes[x * channels..][..channels];
            let (red, green, blue, alpha) = match channels {
                1 => (pixel[0], pixel[0], pixel[0], 255),
                2 => (pixel[0], pixel[0], pixel[0], pixel[1]),
                3 => (pixel[0], pixel[1], pixel[2], 255),
                _ => (pixel[0], pixel[1], pixel[2], pixel[3]),
            };
            let alpha = u64::from(alpha);
            let column = source_column(x as u32, source_w, out_w);
            let premultiply = |value: u8| u64::from(value) * alpha / 255;
            sums[column][0] += premultiply(red);
            sums[column][1] += premultiply(green);
            sums[column][2] += premultiply(blue);
            sums[column][3] += alpha;
        }
    }
    if let Some((row, rows)) = current {
        flush_row(&mut out, row, &sums, &columns, rows);
    }
    Some(out)
}

fn source_column(x: u32, source_w: u32, out_w: u32) -> usize {
    (u64::from(x) * u64::from(out_w) / u64::from(source_w)) as usize
}

fn flush_row(out: &mut Pixmap, row: u32, sums: &[[u64; 4]], columns: &[u32], rows: u64) {
    let width = out.width() as usize;
    for (column, &source_pixels) in columns.iter().enumerate() {
        let count = rows * u64::from(source_pixels);
        if count == 0 {
            continue;
        }
        let average = |sum: u64| ((sum + count / 2) / count) as u8;
        let alpha = average(sums[column][3]);
        let channel = |sum: u64| average(sum).min(alpha);
        let pixel = PremultipliedColorU8::from_rgba(
            channel(sums[column][0]),
            channel(sums[column][1]),
            channel(sums[column][2]),
            alpha,
        );
        if let Some(pixel) = pixel {
            out.pixels_mut()[row as usize * width + column] = pixel;
        }
    }
}

/// Decode a JPEG at the smallest DCT scale covering the target box, so a 4K
/// source never allocates its full buffers; CMYK and 16-bit grey decline.
fn decode_jpeg<R: Read>(reader: R, size: u32) -> Option<Pixmap> {
    let mut decoder = jpeg_decoder::Decoder::new(reader);
    decoder.read_info().ok()?;
    let info = decoder.info()?;
    let (width, height) = (f32::from(info.width), f32::from(info.height));
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let scale = (size as f32 / width).min(size as f32 / height).min(1.0);
    let requested = |side: f32| (side * scale).ceil().clamp(1.0, 65535.0) as u16;
    decoder.scale(requested(width), requested(height)).ok()?;
    let pixels = decoder.decode().ok()?;
    let info = decoder.info()?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    let pixel_count = width as usize * height as usize;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => {
            for pixel in pixels.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        jpeg_decoder::PixelFormat::L8 => {
            for &value in &pixels {
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }
        _ => return None,
    }
    Pixmap::from_vec(rgba, IntSize::from_wh(width, height)?)
}

/// Decode WebP/GIF/BMP through `image` and premultiply into tiny-skia's
/// representation; a GIF and an animated WebP contribute their first frame.
fn decode_image(data: &[u8]) -> Option<Pixmap> {
    let image = image::load_from_memory(data).ok()?.to_rgba8();
    let (width, height) = image.dimensions();
    let mut rgba = image.into_raw();
    let (pixels, _) = rgba.as_chunks_mut::<4>();
    for pixel in pixels {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = ((u32::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    Pixmap::from_vec(rgba, IntSize::from_wh(width, height)?)
}

/// Area-average a large source down, bilinear for upscales and near 1:1; a
/// premultiplied channel interpolates without a transparent edge bleeding.
fn resample(source: PixmapRef, target: &mut Pixmap) {
    let (sw, sh) = (source.width() as f32, source.height() as f32);
    let (tw, th) = (target.width() as f32, target.height() as f32);
    let scale = (tw / sw).min(th / sh);
    let (offset_x, offset_y) = ((tw - sw * scale) / 2.0, (th - sh * scale) / 2.0);

    if scale < 1.0 {
        resample_area(source, target, scale, offset_x, offset_y);
        return;
    }

    let source_pixels = source.pixels();
    let width = source.width() as i32;
    let height = source.height() as i32;
    let target_width = target.width();

    for y in 0..target.height() {
        for x in 0..target_width {
            let fx = (x as f32 + 0.5 - offset_x) / scale - 0.5;
            let fy = (y as f32 + 0.5 - offset_y) / scale - 0.5;
            let (x0, y0) = (fx.floor() as i32, fy.floor() as i32);
            let (tx, ty) = (fx - fx.floor(), fy - fy.floor());

            let mut channels = [0.0_f32; 4];
            for (dx, dy, weight) in [
                (0, 0, (1.0 - tx) * (1.0 - ty)),
                (1, 0, tx * (1.0 - ty)),
                (0, 1, (1.0 - tx) * ty),
                (1, 1, tx * ty),
            ] {
                let (px, py) = (x0 + dx, y0 + dy);
                if px < 0 || py < 0 || px >= width || py >= height || weight == 0.0 {
                    continue;
                }
                let pixel = source_pixels[(py * width + px) as usize];
                channels[0] += f32::from(pixel.red()) * weight;
                channels[1] += f32::from(pixel.green()) * weight;
                channels[2] += f32::from(pixel.blue()) * weight;
                channels[3] += f32::from(pixel.alpha()) * weight;
            }

            // Premultiplied: a colour channel cannot exceed alpha, or
            // `from_rgba` refuses the pixel outright.
            let alpha = channels[3].round().clamp(0.0, 255.0) as u8;
            let channel = |value: f32| value.round().clamp(0.0, f32::from(alpha)) as u8;
            if let Some(pixel) = PremultipliedColorU8::from_rgba(
                channel(channels[0]),
                channel(channels[1]),
                channel(channels[2]),
                alpha,
            ) {
                target.pixels_mut()[(y * target_width + x) as usize] = pixel;
            }
        }
    }
}

/// Average every source pixel whose centre falls in the target pixel's
/// footprint; a four-tap sample of a large downscale reads as mush.
fn resample_area(source: PixmapRef, target: &mut Pixmap, scale: f32, offset_x: f32, offset_y: f32) {
    let source_pixels = source.pixels();
    let width = source.width() as i32;
    let height = source.height() as i32;
    let target_width = target.width();
    let half = 0.5 / scale;

    for y in 0..target.height() {
        let cy = (y as f32 + 0.5 - offset_y) / scale;
        let y0 = ((cy - half - 0.5).ceil() as i32).max(0);
        let y1 = ((cy + half - 0.5).floor() as i32 + 1).min(height);
        if y1 <= y0 {
            continue;
        }
        for x in 0..target_width {
            let cx = (x as f32 + 0.5 - offset_x) / scale;
            let x0 = ((cx - half - 0.5).ceil() as i32).max(0);
            let x1 = ((cx + half - 0.5).floor() as i32 + 1).min(width);
            if x1 <= x0 {
                continue;
            }

            let count = u64::from((x1 - x0) as u32) * u64::from((y1 - y0) as u32);
            let mut channels = [0u64; 4];
            for py in y0..y1 {
                for px in x0..x1 {
                    let pixel = source_pixels[(py * width + px) as usize];
                    channels[0] += u64::from(pixel.red());
                    channels[1] += u64::from(pixel.green());
                    channels[2] += u64::from(pixel.blue());
                    channels[3] += u64::from(pixel.alpha());
                }
            }

            let round = |sum: u64| ((sum + count / 2) / count).min(255) as u8;
            let alpha = round(channels[3]);
            let channel = |sum: u64| round(sum).min(alpha);
            if let Some(pixel) = PremultipliedColorU8::from_rgba(
                channel(channels[0]),
                channel(channels[1]),
                channel(channels[2]),
                alpha,
            ) {
                target.pixels_mut()[(y * target_width + x) as usize] = pixel;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use image::ImageEncoder;

    use super::*;

    #[test]
    fn a_downscale_averages_the_covered_source_area() {
        let mut source = Pixmap::new(4, 2).unwrap();
        for y in 0..2 {
            for x in 0..4 {
                let (red, blue) = if x < 2 { (255, 0) } else { (0, 255) };
                source.pixels_mut()[y * 4 + x] =
                    PremultipliedColorU8::from_rgba(red, 0, blue, 255).unwrap();
            }
        }
        let mut target = Pixmap::new(1, 1).unwrap();
        resample(source.as_ref(), &mut target);

        let pixel = target.pixel(0, 0).unwrap();
        assert_eq!(pixel.alpha(), 255);
        assert!(
            pixel.red() > 100 && pixel.blue() > 100,
            "both halves are represented: {:?}",
            (pixel.red(), pixel.blue())
        );
    }

    #[test]
    fn a_large_jpeg_decodes_at_a_reduced_scale() {
        let mut pixels = Vec::with_capacity(64 * 64 * 3);
        for y in 0..64u8 {
            for x in 0..64u8 {
                pixels.extend_from_slice(&[x * 4, y * 4, 128]);
            }
        }
        let mut data = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut data)
            .write_image(&pixels, 64, 64, image::ExtendedColorType::Rgb8)
            .unwrap();

        let scaled = decode_jpeg(data.as_slice(), 8).expect("a JPEG decodes");
        assert!(
            scaled.width() <= 16 && scaled.height() <= 16,
            "{}x{}",
            scaled.width(),
            scaled.height()
        );
    }

    #[test]
    fn a_large_png_decodes_at_a_reduced_scale() {
        let mut source = Pixmap::new(64, 64).unwrap();
        source.fill(tiny_skia::Color::from_rgba8(200, 40, 40, 255));
        let data = source.encode_png().unwrap();

        let scaled = decode_png(data.as_slice(), 8).expect("a PNG decodes");
        assert!(
            scaled.width() <= 16 && scaled.height() <= 16,
            "{}x{}",
            scaled.width(),
            scaled.height()
        );
        let pixel = scaled.pixel(0, 0).unwrap();
        assert_eq!((pixel.red(), pixel.green(), pixel.blue()), (200, 40, 40));
    }
}
