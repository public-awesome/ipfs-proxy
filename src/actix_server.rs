use crate::app_context::AppContext;
use crate::config::Dimension;
use actix_files::Files;
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
use mime;
use serde::Deserialize;
use std::net::TcpListener;
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
        )
        .service(Files::new("/static", "static"));

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
            let mut content_type = data
                .content_type
                .unwrap_or_else(|| "application/octet-stream".to_string());

            match data.filename {
                Some(mut filename) => {
                    let width = info
                        .img_width
                        .as_ref()
                        .map(|w| w.parse::<u32>().ok())
                        .flatten()
                        .unwrap_or_default();
                    let height = info
                        .img_height
                        .as_ref()
                        .map(|h| h.parse::<u32>().ok())
                        .flatten()
                        .unwrap_or_default();

                    // Resize was requested
                    if width > 0 && height > 0 {
                        if !ctx
                            .clone()
                            .config
                            .permitted_resize_dimensions
                            .contains(&Dimension { width, height })
                        {
                            return HttpResponse::BadRequest()
                                .body("Requested dimensions are not allowed".to_string());
                        } else {
                            debug!("Resizing to {}x{} is requested", &width, &height);
                            let thumbnail_filename =
                                format!("{}-{}x{}.png", &filename, width, height);

                            if !std::path::Path::new(&thumbnail_filename).exists() {
                                debug!("Resizing image {} to {}x{}", &filename, &width, &height);
                                match image::open(&filename) {
                                    Err(error) => {
                                        error!("Couldn't open file {}: {error}", &filename)
                                    }
                                    Ok(img) => {
                                        let thumbnail = img.resize(
                                            width,
                                            height,
                                            image::imageops::FilterType::Lanczos3,
                                        );

                                        thumbnail
                                            .save(&thumbnail_filename)
                                            .expect("Saving image failed");
                                    }
                                }
                            }
                            filename = thumbnail_filename;
                            content_type = "image/png".to_string();
                        }
                    }

                    let mime_type = content_type
                        .parse()
                        .unwrap_or(mime::APPLICATION_OCTET_STREAM);
                    let file = actix_files::NamedFile::open_async(&filename)
                        .await
                        .unwrap()
                        .disable_content_disposition()
                        .set_content_type(mime_type);

                    let mut response = file.into_response(&req);
                    if let Ok(dim) = size(&filename) {
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
                    }

                    debug!("Streaming data {} from {}", &content_type, &filename);

                    response
                }
                None => HttpResponse::BadRequest().body("Error, no data.".to_string()),
            }
        }
    }
}
