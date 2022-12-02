use async_recursion::async_recursion;
use futures::StreamExt;
use sea_orm::entity::prelude::*;
use std::io::prelude::*;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tempfile::Builder;
use tokio::fs;
use tracing::debug;

use crate::ipfs_client::check_ipfs_url;
use crate::AppContext;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Data {
    pub content_type: Option<String>,
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
            filename: Some(filename.to_string()),
        };

        return Ok(Some(data));
    }

    if !ipfs_url.ends_with('/') {
        return get_caching(ctx, &format!("{ipfs_url}/")).await;
    }

    Ok(None)
}

pub async fn set_stream_caching(
    ctx: Arc<AppContext>,
    ipfs_url: &str,
    content_type: Option<String>,
    mut stream: Pin<Box<impl futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>>>>,
) -> Result<Data, anyhow::Error> {
    let filename = caching_filename(
        ipfs_url,
        &ctx.config.ipfs_cache_directory,
        content_type.clone(),
        true,
    )
    .await?;

    let mut tmp_file = Builder::new()
        .prefix(&format!("{}/", &ctx.config.ipfs_cache_directory))
        .tempfile()?;

    while let Some(bytes) = stream.next().await {
        match bytes {
            Err(error) => {
                return Err(error.into());
            }
            Ok(bytes) => {
                debug!("Reading {} bytes to file {}", bytes.len(), &filename);
                tmp_file.write_all(bytes.as_ref())?;
            }
        }
    }

    fs::rename(&tmp_file, &filename).await?;
    drop(tmp_file);

    Ok(Data {
        content_type,
        filename: Some(filename),
    })
}

pub async fn caching_filename(
    ipfs_url: &str,
    directory: &str,
    content_type: Option<String>,
    create: bool,
) -> Result<String, anyhow::Error> {
    let base_uri = check_ipfs_url(ipfs_url)?;

    let mut splits = base_uri.split('/').collect::<Vec<&str>>();
    splits.insert(0, directory);

    // If url ends with `/` we know it's a directory
    let mut is_directory = base_uri.ends_with('/');

    if !is_directory {
        if let Some(content_type) = content_type {
            if content_type == "text/html" {
                // If the file has no extension and is HTML, we know it's a directory listing
                if let Some(filename) = splits.last() {
                    let mimes = mime_guess::from_path(filename);
                    if mimes.is_empty() {
                        is_directory = true;
                    }
                }
            }
        }
    }

    let mut cache_dir = splits.join("/");

    let filename = if is_directory {
        format!("{cache_dir}/index.html")
    } else {
        let filename = splits.pop().expect("Should have an element");
        cache_dir = splits.join("/");
        format!("{cache_dir}/{filename}")
    };

    if create {
        debug!("creating {cache_dir} from {:?}", splits);
        fs::create_dir_all(&cache_dir).await?;
    }

    Ok(filename)
}

/// Remove caching and parent directories if empty
pub async fn delete_caching(ctx: Arc<AppContext>, ipfs_url: &str) -> Result<(), anyhow::Error> {
    let filename =
        caching_filename(ipfs_url, &ctx.config.ipfs_cache_directory, None, false).await?;

    fs::remove_file(&filename).await.ok();

    let mut path = Path::new(&filename).parent();

    while path.is_some() {
        let dir = path.unwrap();
        match dir.read_dir() {
            Err(_) => break,
            Ok(mut files) => {
                if files.next().is_some() {
                    break;
                }
                fs::remove_dir(&dir).await.ok();
            }
        }

        path = dir.parent();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::ipfs_client::fetch_ipfs_data;

    use super::*;

    async fn delete_dir(ctx: Arc<AppContext>) {
        fs::remove_dir_all(&ctx.config.ipfs_cache_directory)
            .await
            .ok();
    }

    #[tokio::test]
    async fn filename_for_dir() -> Result<(), anyhow::Error> {
        let ctx = Arc::new(AppContext::build().await);
        delete_dir(ctx.clone()).await;

        let filename = caching_filename(
            "ipfs://bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344",
            "tmp/ipfs",
            Some("text/html".to_string()),
            true,
        )
        .await?;

        assert_eq!(
            filename,
            "tmp/ipfs/bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/index.html"
        );

        assert!(
            Path::new("tmp/ipfs/bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344")
                .is_dir()
        );

        Ok(())
    }

    #[tokio::test]
    async fn filename_for_subdir() -> Result<(), anyhow::Error> {
        let ctx = Arc::new(AppContext::build().await);
        delete_dir(ctx.clone()).await;

        let filename = caching_filename(
            "ipfs://bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata",
            "tmp/ipfs",
            Some("text/html".to_string()),
            true,
        )
        .await?;

        assert_eq!(
            filename,
            "tmp/ipfs/bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata/index.html"
        );

        assert!(Path::new(
            "tmp/ipfs/bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata"
        )
        .is_dir());

        Ok(())
    }

    #[tokio::test]
    async fn filename_for_non_html_file() -> Result<(), anyhow::Error> {
        let ctx = Arc::new(AppContext::build().await);
        delete_dir(ctx.clone()).await;

        let filename = caching_filename(
            "ipfs://bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata/3",
            "tmp/ipfs",
            Some("application/json".to_string()),
            true,
        )
        .await?;

        assert_eq!(
            filename,
            "tmp/ipfs/bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata/3"
        );

        assert!(Path::new(
            "tmp/ipfs/bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata"
        )
        .is_dir());

        Ok(())
    }

    #[tokio::test]
    async fn filename_for_html_file_without_extension() -> Result<(), anyhow::Error> {
        let ctx = Arc::new(AppContext::build().await);
        delete_dir(ctx.clone()).await;

        let filename = caching_filename(
            "ipfs://bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata/4",
            "tmp/ipfs",
            Some("text/html".to_string()),
            true,
        )
        .await?;

        assert_eq!(
            filename,
            "tmp/ipfs/bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata/4/index.html"
        );

        assert!(Path::new(
            "tmp/ipfs/bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata/4"
        )
        .is_dir());

        Ok(())
    }

    #[tokio::test]
    async fn filename_for_html_file_with_extension() -> Result<(), anyhow::Error> {
        let ctx = Arc::new(AppContext::build().await);
        delete_dir(ctx.clone()).await;

        let filename = caching_filename(
            "ipfs://bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata/5.html",
            "tmp/ipfs",
            Some("text/html".to_string()),
            true,
        )
        .await?;

        assert_eq!(
            filename,
            "tmp/ipfs/bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata/5.html"
        );

        assert!(Path::new(
            "tmp/ipfs/bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata/"
        )
        .is_dir());

        Ok(())
    }

    #[tokio::test]
    async fn delete_caching_one_file() -> Result<(), anyhow::Error> {
        let ctx = Arc::new(AppContext::build().await);

        delete_dir(ctx.clone()).await;

        let ipfs_url =
            "ipfs://bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata/81";

        fetch_ipfs_data(ctx.clone(), ipfs_url).await?;

        super::delete_caching(ctx, ipfs_url).await?;

        Ok(())
    }

    #[tokio::test]
    async fn delete_caching_multiple_files() -> Result<(), anyhow::Error> {
        let ctx = Arc::new(AppContext::build().await);

        delete_dir(ctx.clone()).await;

        let ipfs_url =
            "ipfs://bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata/81";

        fetch_ipfs_data(ctx.clone(), ipfs_url).await?;
        let ipfs_url =
            "ipfs://bafybeicugp6ayh2wh3j2dwb2bhesmxmo2husbbs5prla4wj6rf3ivg3344/metadata/82";

        fetch_ipfs_data(ctx.clone(), ipfs_url).await?;

        super::delete_caching(ctx, ipfs_url).await?;

        Ok(())
    }
}
