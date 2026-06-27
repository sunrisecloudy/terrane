const DEFAULT_ADDR: &str = "127.0.0.1:8780";

#[derive(Clone, Debug)]
pub struct Args {
    pub addr: String,
    pub live_reload: bool,
}

pub fn parse_args() -> Args {
    let args: Vec<String> = std::env::args().collect();
    let addr = args
        .windows(2)
        .find(|w| w[0] == "--addr")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| DEFAULT_ADDR.to_string());
    Args {
        addr,
        live_reload: !args.iter().any(|arg| arg == "--no-live-reload"),
    }
}

pub fn is_loopback(addr: &str) -> bool {
    let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr);
    let host = host.trim_matches(|c| c == '[' || c == ']');
    matches!(host, "::1" | "localhost") || host.starts_with("127.")
}
