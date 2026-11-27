//! Local project logos and their initials fallback in the sidebar.

use std::{
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui_kit::*;
use gpui_kit::component::ActiveTheme as _;
use resvg::{tiny_skia, usvg};

use crate::style::size::TEXT;
const MAX_LOGO_BYTES: u64 = 4 * 1024 * 1024;
const MAX_DIRECTORY_DEPTH: usize = 5;
const THUMBNAIL_SIZE: u32 = 72;

pub struct ProjectLogo;

impl Asset for ProjectLogo {
    type Source = PathBuf;
    type Output = Option<Arc<RenderImage>>;

    fn load(project: PathBuf, _: &mut App) -> impl Future<Output = Self::Output> + Send + 'static {
        async move {
            logo_candidates(&project)
                .into_iter()
                .find_map(|(path, format)| load_logo(&path, format))
        }
    }
}

fn load_logo(path: &Path, format: ImageFormat) -> Option<Arc<RenderImage>> {
    let mut pixels = decode_logo(path, format)?;
    // GPUI textures use BGRA.
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let frames = smallvec::SmallVec::from_buf([image::Frame::new(pixels)]);
    Some(Arc::new(RenderImage::new(frames)))
}

/// The project's logo as the sidebar shows it, encoded as a PNG: what another
/// device (the phone) shows for this project (`sync`'s `projects.logo`).
pub fn logo_png(project: &Path) -> Option<Vec<u8>> {
    let pixels = logo_candidates(project)
        .into_iter()
        .find_map(|(path, format)| decode_logo(&path, format))?;
    let mut png = Vec::new();
    pixels.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(png)
}

fn decode_logo(path: &Path, format: ImageFormat) -> Option<image::RgbaImage> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_LOGO_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_LOGO_BYTES {
        return None;
    }
    if format == ImageFormat::Svg {
        return render_svg(&bytes);
    }
    let format = image::ImageFormat::from_extension(format.extension())?;
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(96 * 1024 * 1024);
    reader.limits(limits);
    Some(reader.decode().ok()?.thumbnail(THUMBNAIL_SIZE, THUMBNAIL_SIZE).into_rgba8())
}

fn render_svg(bytes: &[u8]) -> Option<image::RgbaImage> {
    // Logos are discovered automatically. SVGs can embed images but cannot open
    // other files through external references.
    let options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            ..Default::default()
        },
        ..Default::default()
    };
    let tree = usvg::Tree::from_data(bytes, &options).ok()?;
    let size = tree.size();
    let thumbnail = tiny_skia::Size::from_wh(THUMBNAIL_SIZE as f32, THUMBNAIL_SIZE as f32)?;
    let target = size.scale_to(thumbnail).to_int_size();
    let mut pixmap = tiny_skia::Pixmap::new(target.width(), target.height())?;
    let transform = tiny_skia::Transform::from_scale(
        target.width() as f32 / size.width(),
        target.height() as f32 / size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect();
    image::RgbaImage::from_raw(target.width(), target.height(), rgba)
}

fn logo_candidates(project: &Path) -> Vec<(PathBuf, ImageFormat)> {
    let project_name = project
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let project_name = project_name.to_lowercase().replace('_', "-");
    let base_name = ["-desktop", "-web", "-app"]
        .into_iter()
        .find_map(|suffix| project_name.strip_suffix(suffix))
        .unwrap_or(&project_name);
    let named_logos = [project_name.as_str(), base_name]
        .into_iter()
        .filter(|name| !name.is_empty())
        .flat_map(|name| {
            [
                name.to_owned(),
                format!("{name}-logo"),
                format!("{name}-icon"),
                format!("logo-{name}"),
            ]
        })
        .collect::<Vec<_>>();
    let generic_names = [
        "logo",
        "app-logo",
        "logomark",
        "logo-mark",
        "app-icon",
        "appicon",
        "icon",
        "favicon",
    ];

    let mut directories = vec![(project.to_path_buf(), 0)];
    let mut candidates = Vec::new();
    while let Some((dir, depth)) = directories.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if depth < MAX_DIRECTORY_DEPTH
                    && !matches!(
                        name.as_ref(),
                        "node_modules"
                            | "target"
                            | "vendor"
                            | "build"
                            | "dist"
                            | "generated"
                            | "test"
                            | "tests"
                            | "examples"
                    )
                    && (!name.starts_with('.') || name == ".github")
                {
                    directories.push((path, depth + 1));
                }
            } else if kind.is_file() {
                let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                    continue;
                };
                let stem = stem.to_lowercase().replace('_', "-");
                let priority = if named_logos.contains(&stem) {
                    0
                } else if let Some(index) = generic_names.iter().position(|name| *name == stem) {
                    index + 1
                } else {
                    continue;
                };
                let format = match path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref()
                {
                    Some("png") => ImageFormat::Png,
                    Some("svg") => ImageFormat::Svg,
                    Some("webp") => ImageFormat::Webp,
                    Some("jpg" | "jpeg") => ImageFormat::Jpeg,
                    Some("ico") => ImageFormat::Ico,
                    _ => continue,
                };
                candidates.push((priority, depth, path, format));
            }
        }
    }
    candidates.sort_unstable_by(|a, b| (&a.0, &a.1, &a.2).cmp(&(&b.0, &b.1, &b.2)));
    candidates
        .into_iter()
        .map(|(_, _, path, format)| (path, format))
        .collect()
}

#[derive(IntoElement)]
pub struct ProjectAvatar {
    pub project: PathBuf,
    pub name: SharedString,
}

impl RenderOnce for ProjectAvatar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let size = avatar_size(cx);
        if let Some(Some(logo)) = window.use_asset::<ProjectLogo>(&self.project, cx) {
            return img(logo)
                .size(size)
                .flex_none()
                .rounded(px(5.))
                .object_fit(ObjectFit::Contain)
                .into_any_element();
        }
        render_initials(&self.name, size)
    }
}

/// The first letters of two words ("codeany-desktop" → "CD"), or the
/// first and last of a single word ("nex" → "NX").
fn project_initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| c == '-' || c == '_' || c == '.' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .collect();
    let first = |w: &str| w.chars().next();
    let chars: Vec<char> = match words.as_slice() {
        [] => vec![],
        [one] => {
            let cs: Vec<char> = one.chars().filter(|c| c.is_alphanumeric()).collect();
            match cs.len() {
                0 => vec![],
                1 => vec![cs[0]],
                _ => vec![cs[0], cs[cs.len() - 1]],
            }
        }
        [a, b, ..] => first(a).into_iter().chain(first(b)).collect(),
    };
    let out: String = chars.into_iter().collect::<String>().to_uppercase();
    if out.is_empty() { "?".into() } else { out }
}

pub fn avatar_size(cx: &App) -> Pixels {
    (cx.theme().font_size * 1.55).max(px(18.))
}

fn render_initials(name: &str, size: Pixels) -> AnyElement {
    let hash = name
        .bytes()
        .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    let hue = (hash % 360) as f32 / 360.;
    let fg = hsla(hue, 0.65, 0.45, 1.);
    let bg = hsla(hue, 0.65, 0.55, 0.18);
    let initials = project_initials(name);
    div()
        .flex_none()
        .size(size)
        .rounded(px(5.))
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .text_size(TEXT)
        .font_weight(FontWeight::BOLD)
        .text_color(fg)
        .child(initials)
        .into_any_element()
}
