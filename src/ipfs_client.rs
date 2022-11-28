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
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use crate::app_context::AppContext;
use crate::caching::Data;
use crate::caching::{get_caching, set_caching};

lazy_static! {
    static ref BLOCKED_GATEWAYS: tokio::sync::Mutex<DashMap<String, DateTime<Utc>>> =
        Default::default();
}

#[tracing::instrument(skip_all)]
pub async fn fetch_ipfs_data(ctx: Arc<AppContext>, ipfs_url: &str) -> Result<Data, anyhow::Error> {
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

    match get_caching(ctx.clone(), ipfs_url).await {
        Err(error) => {
            error!("Error while looking for cached data: {error}");
        }
        Ok(cached_data) => {
            if let Some(cached_data) = cached_data {
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
                    .connect_timeout(std::time::Duration::from_millis(20000))
                    .timeout(std::time::Duration::from_millis(20000))
                    .build()?;
                let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
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

                        let bytes = response.bytes().await?;
                        set_caching(ctx.clone(), ipfs_url, &bytes).await?;

                        let result = Data {
                            content_type,
                            bytes: Some(bytes),
                        };

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
