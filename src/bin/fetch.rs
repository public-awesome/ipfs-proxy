use clap::Parser;
use ipfs_proxy::telemetry::{get_subscriber, init_subscriber};
use ipfs_proxy::{ipfs_client, AppContext};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};
use tracing::{error, info};

#[derive(Parser, Debug)]
#[clap(author, version)]
#[clap(about = "This will fetch every IPFS url from the file. One url per line.")]
struct Args {
    #[clap(short, long, value_parser)]
    file: String,

    #[clap(short, long, value_parser)]
    threads_count: Option<usize>,
}

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    let args = Args::parse();

    let subscriber = get_subscriber("info");
    init_subscriber(subscriber);

    let ctx = Arc::new(AppContext::build().await);

    // how many parallel requests at a time
    let sem = Arc::new(Semaphore::new(args.threads_count.unwrap_or(50)));

    let join_handlers: Arc<Mutex<HashMap<usize, JoinHandle<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    info!(
        "Will fetch urls with {} at a time.",
        args.threads_count.unwrap_or(50)
    );
    if let Ok(lines) = read_lines(args.file) {
        for (index, line) in lines.enumerate() {
            if let Ok(ipfs_url) = line {
                let permit = Arc::clone(&sem).acquire_owned().await;
                let join_handlers_clone = Arc::clone(&join_handlers);
                let mut join_handlers = join_handlers.lock().await;
                let ctx = ctx.clone();

                let join_handler = tokio::spawn(async move {
                    let _permit = permit;

                    match ipfs_client::fetch_ipfs_data(ctx, &ipfs_url).await {
                        Err(error) => {
                            error!("Error fetching {}: {}", &ipfs_url, error);
                        }
                        Ok(_) => {
                            info!("[{}] Fetched {}", &index, &ipfs_url);
                        }
                    }

                    let mut join_handlers_clone = join_handlers_clone.lock().await;
                    join_handlers_clone.remove(&index);
                });

                join_handlers.insert(index, join_handler);
            }
        }
    }

    let join_handlers_lock = join_handlers.lock().await;
    let mut left = join_handlers_lock.len();
    drop(join_handlers_lock);
    info!("{} still running. Waiting.", left);

    // Making sure all fetches are done
    #[allow(while_true)]
    while true {
        let join_handlers = join_handlers.lock().await;
        if join_handlers.is_empty() {
            break;
        }

        if left != join_handlers.len() {
            left = join_handlers.len();

            info!("{} joins left. Waiting", join_handlers.len());
        }
        sleep(Duration::from_millis(500)).await;
    }

    Ok(())
}

// The output is wrapped in a Result to allow matching on errors
// Returns an Iterator to the Reader of the lines of the file.
fn read_lines<P>(filename: P) -> io::Result<io::Lines<io::BufReader<File>>>
where
    P: AsRef<Path>,
{
    let file = File::open(filename)?;
    Ok(io::BufReader::new(file).lines())
}
