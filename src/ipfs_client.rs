use anyhow::anyhow;
use anyhow::Context;
use askama::Template;
use chrono::{DateTime, Utc};
use cid::Cid;
use dashmap::DashMap;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use lazy_static::lazy_static;
use reqwest_middleware::ClientBuilder;
#[allow(unused_imports)]
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use reqwest_tracing::TracingMiddleware;
use std::fs;
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use crate::app_context::AppContext;
use crate::caching::delete_caching;
use crate::caching::get_caching;
use crate::caching::set_caching;
use crate::caching::set_stream_caching;
use crate::caching::Data;
use entity::ipfs_object::update_entry;

lazy_static! {
    static ref BLOCKED_GATEWAYS: tokio::sync::Mutex<DashMap<String, DateTime<Utc>>> =
        Default::default();
}

#[derive(Template)]
#[template(path = "directory_listing.html")]
struct DirectoryListingTemplate {
    base_uri: String,
    files: Vec<(String, String)>,
}

#[tracing::instrument(skip_all)]
pub async fn fetch_ipfs_data(ctx: Arc<AppContext>, ipfs_url: &str) -> Result<Data, anyhow::Error> {
    let base_uri = check_ipfs_url(ipfs_url)?;

    match get_caching(ctx.clone(), ipfs_url).await {
        Err(error) => {
            error!("Error while looking for cached data: {error}");
        }
        Ok(cached_data) => {
            if let Some(cached_data) = cached_data {
                let content_length = cached_data
                    .filename
                    .as_ref()
                    .and_then(|f| fs::metadata(f).map(|t| t.len()).ok())
                    .unwrap_or_default();
                let content_type = cached_data.content_type.clone().unwrap_or_default();
                let ipfs_url = ipfs_url.to_string();

                tokio::spawn(async move {
                    if let Err(error) =
                        update_entry(&ctx.db, &ipfs_url, &content_type, content_length as i64).await
                    {
                        error!("Error updating sqlite: {}", error);
                    }
                });

                debug!("Return cached data");
                return Ok(cached_data);
            }
        }
    }

    if ctx.config.ipfs.enabled {
        match Command::new(ctx.config.ipfs.binary_path.clone())
            .arg("ls")
            .arg("-s")
            .arg("--size=false")
            .arg("--resolve-type=false")
            .arg(&base_uri)
            .output()
        {
            Ok(output) => {
                if output.status.success() {
                    let text = String::from_utf8(output.stdout)?;
                    let files = text
                        .split("\n")
                        .map(|line| {
                            let splits = line.splitn(2, " ").collect::<Vec<&str>>();
                            let cid = splits.first().map(|t| t.to_string()).unwrap_or_default();
                            let filename = splits.last().map(|t| t.to_string()).unwrap_or_default();
                            (cid, filename)
                        })
                        .filter(|file| file.1.is_empty() == false)
                        .collect::<Vec<(String, String)>>();

                    if !files.is_empty() {
                        let template = DirectoryListingTemplate {
                            base_uri: base_uri.clone(),
                            files,
                        };
                        match template.render() {
                            Ok(template) => {
                                return Ok(set_caching(
                                    ctx.clone(),
                                    ipfs_url,
                                    "text/html",
                                    template.into(),
                                )
                                .await?);
                            }
                            Err(error) => {
                                error!("Can't render template: {error}");
                            }
                        }
                    }
                } else {
                    error!("Can't run ipfs ls, result non successful");
                }
            }
            Err(error) => {
                error!("Can't run ipfs ls: {error}");
            }
        }
    }

    // We stop using gateways who gave us a 429 too many requests
    let blocked_gateways = BLOCKED_GATEWAYS.lock().await;

    let urls: Vec<String> = ctx
        .config
        .ipfs_gateways
        .iter()
        .filter(|ipfs_gateway| match blocked_gateways.get(*ipfs_gateway) {
            None => true,
            Some(utc_time) => {
                let diff = Utc::now() - *utc_time;
                diff.num_seconds() >= ctx.config.pause_gateway_seconds
            }
        })
        .map(|ipfs_gateway| format!("{}/{}", ipfs_gateway, base_uri))
        .collect::<Vec<String>>();

    let mut futures = urls
        .clone()
        .into_iter()
        .map(|url| {
            let ctx = ctx.clone();
            tokio::spawn(async move {
                let client = reqwest::ClientBuilder::new()
                    .user_agent(&ctx.config.user_agent.clone())
                    .connect_timeout(std::time::Duration::from_millis(ctx.config.connect_timeout))
                    .timeout(std::time::Duration::from_millis(ctx.config.connect_timeout))
                    .build()?;
                let client_with_middleware = ClientBuilder::new(client)
                    .with(TracingMiddleware::default())
                    .build();

                client_with_middleware.get(url).send().await
            })
        })
        .collect::<FuturesUnordered<JoinHandle<_>>>();

    debug!("fetching {urls:?}");
    let now = Instant::now();
    while let Some(result) = futures.next().await {
        let value = result?; // a potential stream error

        match value {
            Ok(response) => {
                let url = response.url().clone();
                let status = response.status();

                // Some IPFS gateway returns 404 because they don't have the data in cache.
                match status {
                    reqwest::StatusCode::OK => {
                        if let Some(content_length) = response.content_length() {
                            if content_length > ctx.config.max_content_length {
                                return Err(anyhow!(
                                    "File is {} bytes, maximum allowed is {}",
                                    content_length,
                                    ctx.config.max_content_length
                                ));
                            }
                        }

                        let content_type = response
                            .headers()
                            .get(reqwest::header::CONTENT_TYPE)
                            .and_then(|value| value.to_str().ok().map(|t| t.to_string()));

                        let stream = Box::pin(response.bytes_stream());
                        let result =
                            set_stream_caching(ctx.clone(), ipfs_url, content_type, stream).await?;

                        let content_length = result
                            .filename
                            .as_ref()
                            .and_then(|f| fs::metadata(f).map(|t| t.len()).ok())
                            .unwrap_or_default();

                        if content_length > ctx.config.max_content_length {
                            delete_caching(ctx.clone(), ipfs_url).await?;
                            return Err(anyhow!(
                                "File is {} bytes, maximum allowed is {}. Fetched and deleting cached file.",
                                content_length,
                                ctx.config.max_content_length
                            ));
                        }

                        info!(
                            "[{}] [{:.3?}] Fetched {} from {}",
                            status.as_u16(),
                            now.elapsed(),
                            &ipfs_url,
                            &url,
                        );

                        update_entry(
                            &ctx.db,
                            ipfs_url,
                            &result.content_type.clone().unwrap_or_default(),
                            content_length as i64,
                        )
                        .await?;

                        return Ok(result);
                    }
                    reqwest::StatusCode::TOO_MANY_REQUESTS => {
                        if let Some(host) = url.host() {
                            let host = host.to_string();
                            for ipfs_gateway in &ctx.config.ipfs_gateways {
                                if ipfs_gateway.contains(&host) {
                                    error!(
                                        "gateway {} returned 429. Adding to block list",
                                        ipfs_gateway
                                    );
                                    let blocked_gateways = BLOCKED_GATEWAYS.lock().await;

                                    blocked_gateways.insert(ipfs_gateway.clone(), Utc::now());
                                }
                            }
                        }
                    }
                    _ => {
                        debug!(
                            "[{}] [{:.3?}] fetched {url}",
                            status.as_u16(),
                            now.elapsed()
                        );
                    }
                }
            }
            Err(error) => {
                info!("failed fetching: {error}");
            }
        }
    }

    error!("Couldn't fetch any url: {urls:?}");
    Err(anyhow!("Couldn't fetch any url: {urls:?}"))
}

/// Check if the IPFS urls seems correct, return the base uri
pub fn check_ipfs_url(ipfs_url: &str) -> Result<String, anyhow::Error> {
    let ipfs_string = "ipfs://";

    let base_uri = if let Some(stripped) = ipfs_url.strip_prefix(ipfs_string) {
        stripped.to_string()
    } else {
        return Err(anyhow!("Not an IPFS URL: {ipfs_url}"));
    };

    let splits = base_uri.split('/').collect::<Vec<&str>>();
    let first = match splits.first() {
        Some(first) => first,
        None => {
            return Err(anyhow!(
                "Not an IPFS URL: {ipfs_url}, no CID on first split"
            ));
        }
    };

    // Check if CID is good
    Cid::try_from(first.to_string()).with_context(|| format!("CID is invalid for {}", ipfs_url))?;

    Ok(base_uri)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::entity::prelude::*;

    #[tokio::test]
    async fn fetch_json() -> Result<(), anyhow::Error> {
        let ctx = Arc::new(AppContext::build().await);
        let remote_url =
            "ipfs://bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata/1";
        let result = fetch_ipfs_data(ctx.clone(), remote_url).await?;

        let ipfs_object = entity::ipfs_object::Entity::find()
            .filter(entity::ipfs_object::Column::RemoteUrl.eq(remote_url))
            .one(&ctx.db)
            .await?
            .expect("Can't find ipfs object");
        assert_eq!(ipfs_object.content_type, "application/json");

        let expected = Data {
            content_type: Some("application/json".to_string()),
            filename: Some(
                "tmp/ipfs/bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata/1"
                    .to_string(),
            ),
        };
        assert_eq!(result, expected);

        let result = fetch_ipfs_data(
            ctx,
            "ipfs://bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata/1",
        )
        .await?;
        assert_eq!(result, expected);

        Ok(())
    }

    #[tokio::test]
    async fn fetch_large_file() {
        let mut ctx = AppContext::build().await;
        ctx.config.max_content_length = 1;
        let ctx = Arc::new(ctx);

        let remote_url =
            "ipfs://bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata/1";

        let result = fetch_ipfs_data(ctx.clone(), remote_url).await;

        assert_eq!(
            result.err().expect("Expected error").to_string(),
            "File is 1023 bytes, maximum allowed is 1"
        );
    }
}
