use anyhow::anyhow;
use async_recursion::async_recursion;
use bytes::Bytes;
use futures::StreamExt;
use sea_orm::entity::prelude::*;
use std::io::prelude::*;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::fs;
use tracing::debug;

use crate::AppContext;

#[derive(Clone, Debug)]
pub struct Data {
    pub content_type: Option<String>,
    pub bytes: Option<Bytes>,
    pub filename: Option<String>,
}

#[async_recursion]
pub async fn get_caching(
    ctx: Arc<AppContext>,
    ipfs_url: &str,
) -> Result<Option<Data>, anyhow::Error> {
    let filename =
        caching_filename(ipfs_url, &ctx.config.ipfs_cache_directory, None, false).await?;
    let filename = filename.as_str();

    debug!("Looking for {filename}");
    if Path::new(filename).is_file() {
        let bytes = fs::read(filename).await?;

        let object = entity::ipfs_object::Entity::find()
            .filter(entity::ipfs_object::Column::RemoteUrl.eq(ipfs_url))
            .one(&ctx.db)
            .await?;
        let content_type = match object {
            Some(object) => Some(object.content_type),
            None => infer::get(&bytes).map(|k| k.mime_type().to_string()),
        };

        let data = Data {
            content_type,
            bytes: Some(bytes.into()),
            filename: None,
        };

        return Ok(Some(data));
    }

    if !ipfs_url.ends_with('/') {
        return get_caching(ctx, &format!("{ipfs_url}/")).await;
    }

    Ok(None)
}

pub async fn set_caching(
    ctx: Arc<AppContext>,
    ipfs_url: &str,
    data: &Bytes,
) -> Result<(), anyhow::Error> {
    let filename =
        caching_filename(ipfs_url, &ctx.config.ipfs_cache_directory, Some(data), true).await?;
    let filename = filename.as_str();

    let mut tmp_file = NamedTempFile::new()?;
    tmp_file.write_all(data.as_ref())?;

    fs::rename(tmp_file, filename).await?;

    Ok(())
}

pub fn set_stream_caching(
    ctx: Arc<AppContext>,
    ipfs_url: &str,
    stream: impl futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>>,
) -> Result<(), anyhow::Error> {
    // stream.for_each(|item| {

    // });
    // for data in stream.next() {}

    Ok(())
}

async fn caching_filename(
    ipfs_url: &str,
    directory: &str,
    data: Option<&Bytes>,
    create: bool,
) -> Result<String, anyhow::Error> {
    let ipfs_string = "ipfs://";

    let base_uri = if let Some(stripped) = ipfs_url.strip_prefix(ipfs_string) {
        stripped.to_string()
    } else {
        return Err(anyhow!("Not an IPFS URL: {ipfs_url}"));
    };

    let mut splits = base_uri.split('/').collect::<Vec<&str>>();
    splits.insert(0, directory);

    let mut is_directory = base_uri.ends_with('/');

    if !is_directory {
        if let Some(data) = data {
            if let Ok(content) = std::str::from_utf8(data) {
                if content.contains("Index of") {
                    is_directory = true;
                }
            }
        }
    }

    if is_directory {
        let cache_dir = splits.join("/");
        let filename = format!("{cache_dir}/index.html");
        debug!("{base_uri} is a directory, creating {cache_dir}");

        if create {
            fs::create_dir_all(cache_dir).await?;
        }

        Ok(filename)
    } else {
        let filename = splits.pop().unwrap();
        let cache_dir = splits.join("/");
        let filename = format!("{cache_dir}/{filename}");
        debug!("{base_uri} is NOT a directory, creating {cache_dir}");

        if create {
            fs::create_dir_all(cache_dir).await?;
        }

        Ok(filename)
    }
}
