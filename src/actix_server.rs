use crate::app_context::AppContext;
use crate::config::Dimension;
use actix_web::http::header;
use actix_web::middleware::Logger;
use actix_web::web::{self, ServiceConfig};
use actix_web::{
    body::MessageBody,
    dev::{Server, ServiceFactory, ServiceRequest, ServiceResponse},
    middleware::Compress,
    App, Error, HttpRequest, HttpResponse, HttpServer, Responder,
};
use imagesize::size;
use magick_rust::{magick_wand_genesis, MagickWand};
use mime;
use serde::Deserialize;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::Once;
use tracing::{debug, error, info};
use tracing_actix_web::TracingLogger;

use crate::ipfs_client;

pub fn run(ctx: AppContext, listener: TcpListener) -> anyhow::Result<Server> {
    let port = listener.local_addr().unwrap().port();
    let ip = listener.local_addr().unwrap().ip();
    let ctx = web::Data::new(ctx);

    let server = HttpServer::new(move || make_app().configure(config_app(ctx.clone())))
        .listen(listener)?
        .run();

    info!("Listening to http://{ip}:{port}/");

    Ok(server)
}

fn config_app(app_ctx: web::Data<AppContext>) -> Box<dyn Fn(&mut ServiceConfig)> {
    Box::new(move |cfg: &mut ServiceConfig| {
        cfg.service(
            web::resource("/ipfs/{ipfs_file:.+}")
                .route(web::get().to(ipfs_file))
                .route(web::head().to(ipfs_file)),
        );

        cfg.app_data(app_ctx.clone());
    })
}

fn make_app() -> App<
    impl ServiceFactory<
        ServiceRequest,
        Response = ServiceResponse<impl MessageBody>,
        Config = (),
        InitError = (),
        Error = Error,
    >,
> {
    App::new()
        .wrap(Logger::default())
        .wrap(TracingLogger::default())
        .wrap(actix_web_opentelemetry::RequestTracing::new())
        .wrap(Compress::default())
}

#[derive(Deserialize)]
struct ImageInfo {
    #[serde(rename(deserialize = "img-width"))]
    img_width: Option<String>,
    #[serde(rename(deserialize = "img-height"))]
    img_height: Option<String>,
    #[serde(rename(deserialize = "img-format"))]
    img_format: Option<String>,
    #[serde(rename(deserialize = "video-format"))]
    video_format: Option<String>,
}

async fn ipfs_file(
    req: HttpRequest,
    ctx: web::Data<AppContext>,
    info: web::Query<ImageInfo>,
) -> impl Responder {
    let ipfs_file = match req.match_info().get("ipfs_file") {
        Some(ipfs_file) => ipfs_file,
        None => {
            let result = HttpResponse::BadRequest().body("Error");

            return result;
        }
    };

    let ipfs_file = format!("ipfs://{ipfs_file}");
    let ctx = ctx.into_inner();

    match ipfs_client::fetch_ipfs_data(ctx.clone(), &ipfs_file).await {
        Err(error) => HttpResponse::BadRequest().body(format!("Error: {error}")),
        Ok(data) => {
            let Some(content_type) = data.content_type else {
                return HttpResponse::BadRequest().body("Can't find file format for the remote IPFS file".to_string());
            };

            match data.filename {
                Some(filename) => match resize_image(ctx, info, filename, content_type) {
                    Ok((filename, content_type)) => {
                        send_filename(&req, filename, content_type).await
                    }
                    Err(error) => {
                        error!("Error: {error}");

                        HttpResponse::BadRequest().body(format!("Error: {error}"))
                    }
                },
                None => HttpResponse::BadRequest().body("Error, no data.".to_string()),
            }
        }
    }
}

async fn send_filename(req: &HttpRequest, filename: String, content_type: String) -> HttpResponse {
    let mime_type = content_type
        .parse()
        .unwrap_or(mime::APPLICATION_OCTET_STREAM);
    let file = actix_files::NamedFile::open_async(&filename)
        .await
        .unwrap()
        .disable_content_disposition()
        .set_content_type(mime_type);

    let mut response = file.into_response(&req);
    let Ok(dim) = size(&filename) else {
        return response;
    };

    debug!("Found dimension for filename {}: {:?}", &filename, &dim);

    let headers = response.headers_mut();

    headers.insert(
        reqwest::header::HeaderName::from_static("x-image-width"),
        reqwest::header::HeaderValue::from_str(&format!("{}", dim.width))
            .expect("Cant convert width to header value"),
    );

    headers.insert(
        reqwest::header::HeaderName::from_static("x-image-height"),
        reqwest::header::HeaderValue::from_str(&format!("{}", dim.height))
            .expect("Cant convert height to header value"),
    );

    headers.insert(
        header::HeaderName::from_static("x-image-size"),
        header::HeaderValue::from_str(&format!("{},{}", dim.width, dim.height))
            .expect("Cant convert width/height to header value"),
    );

    debug!("Streaming data {} from {}", &content_type, &filename);

    response
}

fn resize_image(
    ctx: Arc<AppContext>,
    info: web::Query<ImageInfo>,
    filename: String,
    content_type: String,
) -> Result<(String, String), anyhow::Error> {
    if info.video_format.is_some() {
        return resize_video(ctx, info, filename, content_type);
    }

    let width = info
        .img_width
        .as_ref()
        .map(|w| w.parse::<u32>().ok())
        .flatten();
    let height = info
        .img_height
        .as_ref()
        .map(|h| h.parse::<u32>().ok())
        .flatten();
    let requested_file_format = info
        .img_format
        .as_ref()
        .map(|h| h.to_string())
        .unwrap_or_else(|| "png".to_string());

    let (Some(width), Some(height)) = (width, height) else {
            return Ok((filename, content_type));
        };

    if !ctx
        .clone()
        .config
        .permitted_resize_dimensions
        .contains(&Dimension { width, height })
    {
        return Err(anyhow::anyhow!("Requested dimensions are not allowed"));
    }

    debug!("Resizing to {}x{} is requested", &width, &height);
    let thumbnail_filename = match requested_file_format.as_str() {
        "jpeg" => {
            format!("{}-{}x{}.jpeg", &filename, width, height)
        }
        "mp4" => {
            format!("{}-{}x{}.mp4", &filename, width, height)
        }
        "png" | _ => {
            format!("{}-{}x{}.png", &filename, width, height)
        }
    };

    if !std::path::Path::new(&thumbnail_filename).exists() {
        debug!("Resizing image {} to {}x{}", &filename, &width, &height);
        match image::open(&filename) {
            Err(error) => {
                error!("Couldn't open file {}: {error}", &filename)
            }
            Ok(img) => {
                let thumbnail = img.resize(width, height, image::imageops::FilterType::Lanczos3);

                thumbnail
                    .save(&thumbnail_filename)
                    .expect("Saving image failed");
            }
        }
    }
    let filename = thumbnail_filename;
    let content_type = match requested_file_format.as_str() {
        "jpeg" => "image/jpeg".to_string(),
        "mp4" => "video/mp4".to_string(),
        "png" | _ => "image/png".to_string(),
    };

    Ok((filename, content_type))
}

// Used to make sure MagickWand is initialized exactly once. Note that we
// do not bother shutting down, we simply exit when we're done.
static START: Once = Once::new();

fn resize_video(
    ctx: Arc<AppContext>,
    info: web::Query<ImageInfo>,
    filename: String,
    _content_type: String,
) -> Result<(String, String), anyhow::Error> {
    let width = info
        .img_width
        .as_ref()
        .map(|w| w.parse::<u32>().ok())
        .flatten();
    let height = info
        .img_height
        .as_ref()
        .map(|h| h.parse::<u32>().ok())
        .flatten();
    let requested_file_format = info
        .video_format
        .as_ref()
        .map(|h| h.to_string())
        .unwrap_or_else(|| "png".to_string());

    let extension = match requested_file_format.as_str() {
        "webm" => ".webm",
        "mp4" | _ => ".mp4",
    };
    let mut thumbnail_filename = format!("{}.{}", &filename, &extension);

    if let (Some(width), Some(height)) = (width, height) {
        thumbnail_filename = format!("{}-{}x{}.{}", &filename, width, height, &extension);
    }

    if !std::path::Path::new(&thumbnail_filename).exists() {
        START.call_once(|| {
            magick_wand_genesis();
        });

        let wand = MagickWand::new();

        match wand.read_image(&filename) {
            Err(error) => {
                error!("Couldn't open file {}: {error}", &filename)
            }
            Ok(_) => {
                if let (Some(width), Some(height)) = (width, height) {
                    if !ctx
                        .clone()
                        .config
                        .permitted_resize_dimensions
                        .contains(&Dimension { width, height })
                    {
                        return Err(anyhow::anyhow!("Requested dimensions are not allowed"));
                    }
                    debug!("Resizing video {} to {}x{}", &filename, &width, &height);

                    wand.fit(width as usize, height as usize);
                };

                wand.write_image(&thumbnail_filename)
                    .expect("Saving video failed");
            }
        }
    }

    let filename = thumbnail_filename;
    let content_type = match requested_file_format.as_str() {
        "webm" => "video/webm".to_string(),
        "mp4" | _ => "video/mp4".to_string(),
    };

    Ok((filename, content_type))
}
