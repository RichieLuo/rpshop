//! Rasterize imported normalized Axure vector contours for the image widget.

use std::io::Cursor;

use base64::Engine;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use serde_json::Value;

#[derive(Clone, Copy)]
struct Point {
    x: f64,
    y: f64,
}

#[derive(Clone, Copy)]
struct Edge {
    a: Point,
    b: Point,
}

/// Preserve arbitrary vector appearance as a transparent PNG data URI.
///
/// Coordinates and Bézier handles are normalized to the widget dimensions.
/// Closed contours use even-odd filling, preserving holes in icon glyphs.
/// Returns `None` for malformed paths or work exceeding the bounded renderer.
pub fn rasterize(
    properties: &Value,
    width: f32,
    height: f32,
    fill: &str,
    stroke: &str,
    stroke_width: f32,
) -> Option<String> {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let reduction = (1024.0 / width).min(1024.0 / height).min(1.0);
    let w = (width * reduction).ceil().max(1.0) as u32;
    let h = (height * reduction).ceil().max(1.0) as u32;
    let rw = w * 2;
    let rh = h * 2;
    let contours = properties.get("ControlPoints")?.as_array()?;
    let mut edges = Vec::new();
    let mut fill_edges = Vec::new();
    let mut point_count = 0;
    for contour in contours {
        let points = contour.as_array()?;
        if points.len() < 2 {
            continue;
        }
        point_count += points.len();
        if point_count > 20_000 {
            return None;
        }
        let closed = points.iter().all(|p| {
            p.get("HigherSegmentExists")
                .and_then(Value::as_bool)
                .unwrap_or(true)
        });
        for i in 0..points.len() {
            let current = &points[i];
            if !current
                .get("HigherSegmentExists")
                .and_then(Value::as_bool)
                .unwrap_or(true)
            {
                continue;
            }
            let next = &points[(i + 1) % points.len()];
            let a = point(current.get("Path")?, rw, rh)?;
            let b = point(next.get("Path")?, rw, rh)?;
            let handle_a = current.get("HigherHandle").and_then(|v| point(v, rw, rh));
            let handle_b = next.get("LowerHandle").and_then(|v| point(v, rw, rh));
            let begin = edges.len();
            if handle_a.is_some() || handle_b.is_some() {
                let c = handle_a.unwrap_or(a);
                let d = handle_b.unwrap_or(b);
                let length = distance(a, c) + distance(c, d) + distance(d, b);
                let steps = (length / 3.0).ceil().clamp(4.0, 96.0) as usize;
                let mut previous = a;
                for step in 1..=steps {
                    let t = step as f64 / steps as f64;
                    let s = 1.0 - t;
                    let p = Point {
                        x: s * s * s * a.x
                            + 3.0 * s * s * t * c.x
                            + 3.0 * s * t * t * d.x
                            + t * t * t * b.x,
                        y: s * s * s * a.y
                            + 3.0 * s * s * t * c.y
                            + 3.0 * s * t * t * d.y
                            + t * t * t * b.y,
                    };
                    edges.push(Edge { a: previous, b: p });
                    previous = p;
                }
            } else {
                edges.push(Edge { a, b });
            }
            if closed {
                fill_edges.extend_from_slice(&edges[begin..]);
            }
            if edges.len() > 100_000 {
                return None;
            }
        }
    }
    if edges.is_empty() || edges.len().checked_mul(rh as usize)? > 25_000_000 {
        return None;
    }
    let mut canvas = RgbaImage::new(rw, rh);
    let fill = color(fill)?;
    if fill[3] != 0 {
        fill_contours(&mut canvas, &fill_edges, fill);
    }
    let stroke = color(stroke)?;
    if stroke[3] != 0 && stroke_width.is_finite() && stroke_width > 0.0 {
        let radius = (stroke_width as f64 * reduction as f64).min(64.0);
        stroke_edges(&mut canvas, &edges, stroke, radius)?;
    }
    let image = image::imageops::resize(&canvas, w, h, image::imageops::FilterType::Triangle);
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut png, ImageFormat::Png)
        .ok()?;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png.into_inner())
    ))
}

fn point(value: &Value, width: u32, height: u32) -> Option<Point> {
    let x = value.get("X")?.as_f64()?;
    let y = value.get("Y")?.as_f64()?;
    if !x.is_finite() || !y.is_finite() || x.abs() > 64.0 || y.abs() > 64.0 {
        return None;
    }
    Some(Point {
        x: x * width as f64,
        y: y * height as f64,
    })
}

fn distance(a: Point, b: Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

fn color(text: &str) -> Option<[u8; 4]> {
    if text.eq_ignore_ascii_case("transparent") || text.eq_ignore_ascii_case("none") {
        return Some([0, 0, 0, 0]);
    }
    let hex = text.strip_prefix('#')?;
    if hex.len() == 3 || hex.len() == 4 {
        let bytes = hex.as_bytes();
        let mut result = [0, 0, 0, 255];
        for (i, byte) in bytes.iter().enumerate() {
            result[i] = (*byte as char).to_digit(16)? as u8 * 17;
        }
        return Some(result);
    }
    if hex.len() != 6 && hex.len() != 8 || !hex.is_ascii() {
        return None;
    }
    let mut result = [0, 0, 0, 255];
    for (i, component) in result.iter_mut().enumerate().take(hex.len() / 2) {
        *component = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(result)
}

fn fill_contours(canvas: &mut RgbaImage, edges: &[Edge], color: [u8; 4]) {
    let mut intersections = Vec::with_capacity(edges.len());
    for y in 0..canvas.height() {
        intersections.clear();
        let scan = y as f64 + 0.5;
        for edge in edges {
            if (edge.a.y > scan) != (edge.b.y > scan) {
                intersections.push(
                    edge.a.x + (scan - edge.a.y) * (edge.b.x - edge.a.x) / (edge.b.y - edge.a.y),
                );
            }
        }
        intersections.sort_by(f64::total_cmp);
        for pair in intersections.chunks_exact(2) {
            let first = (pair[0] - 0.5).ceil().max(0.0) as u32;
            let end = (pair[1] - 0.5).ceil().max(0.0).min(canvas.width() as f64) as u32;
            for x in first..end {
                canvas.put_pixel(x, y, Rgba(color));
            }
        }
    }
}

fn stroke_edges(canvas: &mut RgbaImage, edges: &[Edge], color: [u8; 4], radius: f64) -> Option<()> {
    // One mask avoids darkening joins when the stroke itself is translucent.
    let mut mask = vec![false; canvas.width() as usize * canvas.height() as usize];
    let mut work = 0_u64;
    for edge in edges {
        let left = (edge.a.x.min(edge.b.x) - radius)
            .floor()
            .max(0.0)
            .min(canvas.width() as f64) as u32;
        let right = (edge.a.x.max(edge.b.x) + radius)
            .ceil()
            .max(0.0)
            .min(canvas.width() as f64) as u32;
        let top = (edge.a.y.min(edge.b.y) - radius)
            .floor()
            .max(0.0)
            .min(canvas.height() as f64) as u32;
        let bottom = (edge.a.y.max(edge.b.y) + radius)
            .ceil()
            .max(0.0)
            .min(canvas.height() as f64) as u32;
        work += (right - left) as u64 * (bottom - top) as u64;
        if work > 25_000_000 {
            return None;
        }
        let dx = edge.b.x - edge.a.x;
        let dy = edge.b.y - edge.a.y;
        let denominator = dx * dx + dy * dy;
        for y in top..bottom {
            for x in left..right {
                let px = x as f64 + 0.5;
                let py = y as f64 + 0.5;
                let t = if denominator > 0.0 {
                    (((px - edge.a.x) * dx + (py - edge.a.y) * dy) / denominator).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let ex = px - edge.a.x - t * dx;
                let ey = py - edge.a.y - t * dy;
                if ex * ex + ey * ey <= radius * radius {
                    mask[y as usize * canvas.width() as usize + x as usize] = true;
                }
            }
        }
    }
    for (pixel, draw) in canvas.pixels_mut().zip(mask) {
        if draw {
            let source_a = color[3] as f32 / 255.0;
            let dest_a = pixel[3] as f32 / 255.0;
            let alpha = source_a + dest_a * (1.0 - source_a);
            if alpha > 0.0 {
                for i in 0..3 {
                    pixel[i] = ((color[i] as f32 * source_a
                        + pixel[i] as f32 * dest_a * (1.0 - source_a))
                        / alpha)
                        .round() as u8;
                }
                pixel[3] = (alpha * 255.0).round() as u8;
            }
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn contour(coords: &[(f64, f64)]) -> Value {
        Value::Array(
            coords
                .iter()
                .map(|(x, y)| json!({"Path":{"X":x,"Y":y},"HigherSegmentExists":true}))
                .collect(),
        )
    }

    #[test]
    fn preserves_icon_holes_and_transparency() {
        let properties = json!({"ControlPoints": [
            contour(&[(0.0,0.0),(1.0,0.0),(1.0,1.0),(0.0,1.0)]),
            contour(&[(0.25,0.25),(0.75,0.25),(0.75,0.75),(0.25,0.75)])
        ]});
        let data = rasterize(&properties, 40.0, 40.0, "#ff0000", "transparent", 0.0).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.split_once(',').unwrap().1)
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap().into_rgba8();
        assert_eq!(image.dimensions(), (40, 40));
        assert_eq!(image.get_pixel(20, 20)[3], 0);
        assert_eq!(image.get_pixel(4, 4).0, [255, 0, 0, 255]);
    }

    #[test]
    fn rejects_invalid_sizes_and_paths() {
        assert!(rasterize(&json!({}), 40.0, 40.0, "#000000", "#000000", 1.0).is_none());
        assert!(rasterize(&json!({}), f32::NAN, 40.0, "#000000", "#000000", 1.0).is_none());
        assert!(
            rasterize(
                &json!({"ControlPoints":[]}),
                40.0,
                40.0,
                "#000000",
                "#000000",
                1.0
            )
            .is_none()
        );
    }
}
