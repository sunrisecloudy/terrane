use std::env;
use std::path::PathBuf;

use terrane_app_build::BuildOptions;

fn main() {
    if let Err(e) = run() {
        eprintln!("terrane-app-build: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let first = args
        .next()
        .ok_or("usage: terrane-app-build [--check] <app-dir>")?;
    let (check_only, app_dir) = if first == "--check" {
        let app_dir = args
            .next()
            .map(PathBuf::from)
            .ok_or("usage: terrane-app-build [--check] <app-dir>")?;
        (true, app_dir)
    } else {
        (false, PathBuf::from(first))
    };

    let result = terrane_app_build::build_app(BuildOptions {
        app_dir,
        check_only,
    })?;

    if check_only {
        println!("checked {}", result.dist.display());
    } else {
        println!("built {}", result.dist.display());
    }
    Ok(())
}
