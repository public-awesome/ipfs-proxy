use anyhow::anyhow;
use bytes::Bytes;
use std::io::prelude::*;
use std::path::Path;
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::fs;

use crate::AppContext;

#[derive(Clone, Debug)]
pub struct Data {
    pub content_type: Option<String>,
    pub bytes: Option<Bytes>,
}

pub async fn get_caching(
    ctx: Arc<AppContext>,
    ipfs_url: &str,
) -> Result<Option<Data>, anyhow::Error> {
    let filename = caching_filename(ipfs_url, &ctx.config.ipfs_cache_directory).await?;
    let filename = filename.as_str();

    if Path::new(filename).exists() {
        let bytes = fs::read(filename).await?;

        let content_type = infer::get(&bytes).map(|k| k.mime_type().to_string());

        let data = Data {
            content_type,
            bytes: Some(bytes.into()),
        };

        return Ok(Some(data));
    }

    Ok(None)
}

pub async fn set_caching(
    ctx: Arc<AppContext>,
    ipfs_url: &str,
    data: &Bytes,
) -> Result<(), anyhow::Error> {
    let filename = caching_filename(ipfs_url, &ctx.config.ipfs_cache_directory).await?;
    let filename = filename.as_str();

    let mut tmp_file = NamedTempFile::new()?;
    tmp_file.write_all(data.as_ref())?;

    fs::rename(tmp_file, filename).await?;

    Ok(())
}

async fn caching_filename(ipfs_url: &str, directory: &str) -> Result<String, anyhow::Error> {
    let ipfs_string = "ipfs://";

    let base_uri = if let Some(stripped) = ipfs_url.strip_prefix(ipfs_string) {
        stripped.to_string()
    } else {
        return Err(anyhow!("Not an IPFS URL: {ipfs_url}"));
    };

    let mut splits = base_uri.split('/').collect::<Vec<&str>>();
    splits.insert(0, directory);

    if base_uri.ends_with('/') {
        let cache_dir = splits.join("/");
        let filename = format!("{cache_dir}/index.html");

        fs::create_dir_all(cache_dir).await?;

        Ok(filename)
    } else {
        let filename = splits.pop().unwrap();
        let cache_dir = splits.join("/");
        let filename = format!("{cache_dir}/{filename}");

        fs::create_dir_all(cache_dir).await?;

        Ok(filename)
    }
}
