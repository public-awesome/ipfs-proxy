use anyhow::anyhow;
use anyhow::Context;
use chrono::{DateTime, Utc};
use cid::Cid;
use dashmap::DashMap;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use lazy_static::lazy_static;
use reqwest_middleware::ClientBuilder;
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use reqwest_tracing::TracingMiddleware;
use std::io::prelude::*;
use std::sync::Arc;
use std::time::Instant;
use tempfile::NamedTempFile;
use tokio::fs;
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use crate::app_context::AppContext;
use crate::caching::caching_filename;
use crate::caching::get_caching;
use crate::caching::set_stream_caching;
use crate::caching::Data;
use entity::ipfs_object::update_entry;

lazy_static! {
    static ref BLOCKED_GATEWAYS: tokio::sync::Mutex<DashMap<String, DateTime<Utc>>> =
        Default::default();
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
                update_entry(
                    &ctx.db,
                    ipfs_url,
                    &cached_data
                        .content_type
                        .as_ref()
                        .map(|ct| ct.to_owned())
                        .unwrap_or_default(),
                    cached_data
                        .bytes
                        .as_ref()
                        .map(|b| b.len())
                        .unwrap_or_default() as i64,
                )
                .await?;

                info!("Return cached data");
                return Ok(cached_data);
            }
        }
    }

    // We stop using gateways who gave us a 429 too many requests
    let blocked_gateways = BLOCKED_GATEWAYS.lock().await;
    let blocked_minutes = 2;

    let urls: Vec<String> = ctx
        .config
        .ipfs_gateways
        .iter()
        .filter(|ipfs_gateway| match blocked_gateways.get(*ipfs_gateway) {
            None => true,
            Some(utc_time) => {
                let diff = Utc::now() - *utc_time;
                diff.num_minutes() >= blocked_minutes
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
                let retry_policy =
                    ExponentialBackoff::builder().build_with_max_retries(ctx.config.max_retries);
                let client_with_middleware = ClientBuilder::new(client)
                    .with(TracingMiddleware::default())
                    .with(RetryTransientMiddleware::new_with_policy(retry_policy))
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
                        info!(
                            "[{}] [{:.3?}] fetched {url}",
                            status.as_u16(),
                            now.elapsed(),
                        );

                        let content_type = response
                            .headers()
                            .get(reqwest::header::CONTENT_TYPE)
                            .and_then(|value| value.to_str().ok().map(|t| t.to_string()));

                        let stream = Box::pin(response.bytes_stream());
                        return Ok(set_stream_caching(ctx, ipfs_url, content_type, stream).await?);

                        // let mut tmp_file = NamedTempFile::new()?;

                        // let filename = caching_filename(
                        //     ipfs_url,
                        //     &ctx.config.ipfs_cache_directory,
                        //     None,
                        //     true,
                        // )
                        // .await?;

                        // let mut stream = response.bytes_stream();

                        // while let Some(bytes) = stream.next().await {
                        //     match bytes {
                        //         Err(error) => {
                        //             return Err(error.into());
                        //         }
                        //         Ok(bytes) => {
                        //             debug!("Reading {} bytes to file {}", bytes.len(), &filename);
                        //             tmp_file.write_all(bytes.as_ref())?;
                        //         }
                        //     }
                        // }

                        // fs::rename(&tmp_file, &filename).await?;
                        // drop(tmp_file);

                        // let result = Data {
                        //     content_type: content_type.clone(),
                        //     bytes: None,
                        //     filename: Some(filename),
                        // };

                        // return Ok(result);
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
fn check_ipfs_url(ipfs_url: &str) -> Result<String, anyhow::Error> {
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
