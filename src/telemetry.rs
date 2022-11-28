use tracing::{subscriber::set_global_default, Subscriber};

#[allow(unused_imports)]
use tracing_subscriber::{fmt::format::FmtSpan, layer::SubscriberExt, EnvFilter, Registry};

pub fn get_subscriber(level: &str) -> impl Subscriber + Send + Sync {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    env_logger::init_from_env(env_logger::Env::new().default_filter_or(level));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_level(true)
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .compact();

    Registry::default().with(env_filter).with(fmt_layer)
}

/// Register a subscriber as global default to process span data.
/// It should only be called once!
pub fn init_subscriber(subscriber: impl Subscriber + Send + Sync) {
    set_global_default(subscriber).expect("Failed to set subscriber");
}
