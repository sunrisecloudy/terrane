const DEFAULT_ADDR: &str = "127.0.0.1:8780";

#[derive(Clone, Debug)]
pub struct Args {
    pub addr: String,
    pub live_reload: bool,
    /// `--apps <dir>` — dev mode: scan this bundle folder on every catalog
    /// request so uncataloged apps show up without an `app add`.
    pub apps_dir: Option<std::path::PathBuf>,
}

pub fn parse_args() -> Args {
    let args: Vec<String> = std::env::args().collect();
    let addr = args
        .windows(2)
        .find(|w| w[0] == "--addr")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| DEFAULT_ADDR.to_string());
    let apps_dir = args
        .windows(2)
        .find(|w| w[0] == "--apps")
        .map(|w| std::path::PathBuf::from(&w[1]));
    Args {
        addr,
        live_reload: !args.iter().any(|arg| arg == "--no-live-reload"),
        apps_dir,
    }
}

pub fn is_loopback(addr: &str) -> bool {
    let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr);
    let host = host.trim_matches(|c| c == '[' || c == ']');
    matches!(host, "::1" | "localhost") || host.starts_with("127.")
}
