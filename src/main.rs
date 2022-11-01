use std::io::Cursor;

use image::GenericImageView;
use tide::prelude::*;
use tracing_subscriber::prelude::*;

const EXIT_CODE_BINDERR: i32 = 1;
const EXIT_CODE_ACCEPTERR: i32 = 2;
const EXIT_CODE_MISSING_UPSTREAM: i32 = 3;

#[derive(Debug, Clone)]
struct State {
    upstream_uri: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
struct QueryParams {
    width: Option<u32>,
    height: Option<u32>,
    fit: Option<FitType>,
    format: Option<String>,
}

impl QueryParams {
    fn has_resize(&self) -> bool {
        self.width.is_some() || self.height.is_some() || self.fit.is_some()
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(try_from = "&str")]
enum FitType {
    Bounds,
    Cover,
    Crop,
}

impl TryFrom<&str> for FitType {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = match value {
            "bounds" => FitType::Bounds,
            "cover" => FitType::Cover,
            "crop" => FitType::Crop,
            _ => anyhow::bail!("Invalid fit type {value}"),
        };

        Ok(value)
    }
}

#[async_std::main]
async fn main() {
    dotenv::dotenv().ok();
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let bind = std::env::var("QUICKLY_BIND")
        .or_else(|_| std::env::var("BIND"))
        .unwrap_or_else(|_| "0.0.0.0:8787".to_string());

    let Ok(upstream) = std::env::var("QUICKLY_UPSTREAM") else {
        eprintln!("`QUICKLY_UPSTREAM` is not set!");
        std::process::exit(EXIT_CODE_MISSING_UPSTREAM);
    };

    let state = State {
        upstream_uri: upstream,
    };

    let mut server = tide::Server::with_state(state);

    server.with(tide_tracing::TraceMiddleware::new());
    server.with(tide_compress::CompressMiddleware::new());

    server.at("/*path").get(transform_image);

    let Ok(mut listener) = server.bind(&bind).await else {
        eprintln!("Unable to bind to {bind}");
        std::process::exit(EXIT_CODE_BINDERR);
    };

    for info in listener.info().iter() {
        println!("Server listening on {info}");
    }

    let Ok(_) = listener.accept().await else {
        eprintln!("Unable to accept incoming connections!");
        std::process::exit(EXIT_CODE_ACCEPTERR);
    };
}

async fn transform_image(req: tide::Request<State>) -> tide::Result {
    let state = req.state();

    let Ok(path) = req.param("path") else {
        return Ok(tide::Response::new(422));
    };

    let query = match req.query() {
        Ok(q) => q,
        _ => QueryParams::default(),
    };

    let mut buffer = surf::get(format!("{}/{}", state.upstream_uri, path))
        .await?
        .body_bytes()
        .await?;

    if query.has_resize() {
        let format = check_format_specified(query.format);
        buffer = resize_image(&buffer, query.width, query.height, query.fit, format)?;
    }

    let res = tide::Response::builder(200)
        .body(tide::Body::from_bytes(buffer))
        .content_type("application/octet-stream")
        .build();

    Ok(res)
}

fn resize_image(
    buffer: &[u8],
    width: Option<u32>,
    height: Option<u32>,
    fit: Option<FitType>,
    format: Option<image::ImageFormat>,
) -> anyhow::Result<Vec<u8>> {
    let src_format = image::guess_format(buffer)?;
    let img = image::load_from_memory(buffer)?;

    let (src_width, src_height) = img.dimensions();
    let filter = image::imageops::Triangle;

    tracing::debug!("Processing image with format {src_format:?}");
    tracing::debug!("Resizing to width {width:?} height {height:?} fit {fit:?}");

    let resized = match (fit, width, height) {
        (Some(FitType::Crop), Some(w), Some(h)) => img.resize_to_fill(w, h, filter),
        (Some(FitType::Bounds), Some(w), Some(h)) => img.resize(w, h, filter),
        (Some(FitType::Cover), Some(width), Some(height)) => {
            if width > height {
                img.resize((height / src_height) * width, height, filter)
            } else {
                img.resize(width, (width / src_width) * height, filter)
            }
        }
        _ => match (width, height) {
            (None, None) => img,
            (Some(w), None) => {
                let h = (w as f32 / src_width as f32) * src_height as f32;

                img.resize(w, h.round() as u32, filter)
            }
            (None, Some(h)) => img.resize((h / src_height) * src_width, h, filter),
            (Some(w), Some(h)) => img.resize_exact(w, h, filter),
        },
    };

    let (dst_width, dst_height) = resized.dimensions();
    let dst_format = format.unwrap_or(src_format);

    tracing::debug!("Writing as {dst_format:?}");
    tracing::debug!("Resized from {src_width}x{src_height} to {dst_width}x{dst_height}");

    let buffer = Vec::new();
    let mut cursor = Cursor::new(buffer);

    resized.write_to(&mut cursor, dst_format)?;

    Ok(cursor.into_inner())
}

fn check_format_specified(format: Option<String>) -> Option<image::ImageFormat> {
    let Some(extension) = format else {
        return None;
    };

    let format = match extension.as_ref() {
        "jpg" => image::ImageFormat::Jpeg,
        "jpeg" => image::ImageFormat::Jpeg,
        "webp" => image::ImageFormat::WebP,
        "png" => image::ImageFormat::Png,
        "gif" => image::ImageFormat::Gif,
        _ => return None,
    };

    Some(format)
}
