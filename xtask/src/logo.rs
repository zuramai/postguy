//! `cargo xtask logo`: rasterize the original Postguy mark from `assets/logo/postguy.svg`
//! into the PNG sizes packaging needs (`assets/logo/postguy-<size>.png`). The mark lives in
//! the repo as contributor-original path data + stroke constants; this renders it with
//! tiny-skia at tooling time so no SVG renderer ever ships in any binary.

use anyhow::{Context, Result, bail};
use kurbo::ParamCurve;

/// The package/icon sizes the mark is needed at (M10.3 packaging grows this list).
const SIZES: &[u32] = &[16, 32, 64, 128, 256, 512];
const STROKE_WIDTH: f32 = 2.0;
/// Logo colour: the palette's ink (`--text`) from PROMPT.md 6.6, monochrome so the mark works
/// on light and dark. Coloured variants can be generated later from the same source.
const INK: [u8; 4] = [0x1F, 0x23, 0x28, 0xFF];

pub fn run(args: &[String]) -> Result<()> {
    let check = args.iter().any(|a| a == "--check");
    let root = crate::gates::root();
    let logo_dir = root.join("assets").join("logo");
    let svg_path = logo_dir.join("postguy.svg");
    let svg = std::fs::read_to_string(&svg_path).with_context(|| format!("reading {}", svg_path.display()))?;
    let paths = logo_paths(&svg)?;

    let mut problems: Vec<String> = Vec::new();
    for size in SIZES {
        let png_path = logo_dir.join(format!("postguy-{size}.png"));
        match render_png(&paths, *size) {
            Ok(bytes) => {
                if check {
                    match std::fs::read(&png_path) {
                        Ok(existing) if existing == bytes => {}
                        Ok(_) => problems.push(format!("{} differs from the current rendering", png_path.display())),
                        Err(_) => problems.push(format!("{} is missing", png_path.display())),
                    }
                } else {
                    std::fs::write(&png_path, &bytes).with_context(|| format!("writing {}", png_path.display()))?;
                    println!("logo: wrote {} ({} bytes)", png_path.display(), bytes.len());
                }
            }
            Err(err) => problems.push(format!("rendering {size}px: {err:#}")),
        }
    }

    if !problems.is_empty() {
        bail!("logo pipeline problem:\n  {}", problems.join("\n  "));
    }
    if check {
        println!("logo: {} sizes up to date", SIZES.len());
    }
    Ok(())
}

/// Every `<path d>` of the logo source, in document order. Unknown element kinds fail here
/// rather than dropping geometry silently.
fn logo_paths(svg: &str) -> Result<Vec<String>> {
    crate::icons::icon_path_data(svg)
}

/// Rasterize the (stroke-only) mark at `size` px: flatten each path in logo coordinates,
/// stroke it with round caps and joins under a scale transform, onto a transparent pixmap.
fn render_png(paths: &[String], size: u32) -> Result<Vec<u8>> {
    let scale = size as f32 / 24.0;
    let transform = tiny_skia::Transform::from_scale(scale, scale);
    let mut pixmap = tiny_skia::Pixmap::new(size, size).context("allocating pixmap")?;
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(INK[0], INK[1], INK[2], INK[3]);
    paint.anti_alias = true;
    let stroke =
        tiny_skia::Stroke { width: STROKE_WIDTH, line_cap: tiny_skia::LineCap::Round, line_join: tiny_skia::LineJoin::Round, ..Default::default() };

    for d in paths {
        let flat = flatten_path(d).with_context(|| format!("parsing path `{d}`"))?;
        pixmap.stroke_path(&flat, &paint, &stroke, transform, None);
    }

    pixmap.encode_png().context("encoding PNG")
}

/// Flatten one path-data string to a polyline path in the 24-unit logo box (curves sampled at
/// 32 points, far beyond what a 4x worst-case scaling can resolve visually).
fn flatten_path(d: &str) -> Result<tiny_skia::Path> {
    let bez = kurbo::BezPath::from_svg(d).with_context(|| format!("invalid path data `{d}`"))?;
    let mut builder = tiny_skia::PathBuilder::new();
    let mut current = kurbo::Point::ZERO;
    const SAMPLES: i32 = 32;
    for element in bez.iter() {
        match element {
            kurbo::PathEl::MoveTo(p) => {
                current = p;
                builder.move_to(p.x as f32, p.y as f32);
            }
            kurbo::PathEl::LineTo(p) => {
                current = p;
                builder.line_to(p.x as f32, p.y as f32);
            }
            kurbo::PathEl::QuadTo(c, p) => {
                let quad = kurbo::QuadBez::new(current, c, p);
                for k in 1..=SAMPLES {
                    let point = quad.eval(f64::from(k) / f64::from(SAMPLES));
                    builder.line_to(point.x as f32, point.y as f32);
                }
                current = p;
            }
            kurbo::PathEl::CurveTo(c1, c2, p) => {
                let cubic = kurbo::CubicBez::new(current, c1, c2, p);
                for k in 1..=SAMPLES {
                    let point = cubic.eval(f64::from(k) / f64::from(SAMPLES));
                    builder.line_to(point.x as f32, point.y as f32);
                }
                current = p;
            }
            kurbo::PathEl::ClosePath => builder.close(),
        }
    }
    builder.finish().context("empty path")
}
