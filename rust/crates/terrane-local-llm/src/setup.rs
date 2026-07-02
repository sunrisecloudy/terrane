//! MLX runtime provisioning and resolution.
//!
//! The MLX model zoo lives in Python (`mlx-lm`), so the runtime is an
//! installable artifact, not a linked dependency. `setup_mlx` provisions a
//! **pinned, self-contained** runtime under `$TERRANE_HOME/engines/` using
//! `uv` (downloading the uv static binary first if the machine has none), and
//! records what it found or installed in `engines/mlx.json`. Resolution order
//! everywhere: env override → manifest → bare names on PATH.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::download::download_url;
use crate::LlmError;

/// The mlx-lm version this Terrane build was tested against.
pub const MLX_LM_VERSION: &str = "0.31.3";
/// The uv version bootstrapped when the machine has none.
pub const UV_VERSION: &str = "0.11.21";
/// Installed alongside mlx-lm so the worker can token-mask JSON schemas.
pub const LLGUIDANCE_VERSION: &str = "1.7.6";

/// A usable MLX runtime: where its CLIs live and where that knowledge came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlxRuntime {
    pub generate_bin: String,
    pub server_bin: String,
    pub version: Option<String>,
    pub source: RuntimeSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSource {
    /// `TERRANE_MLX_LM_BIN` / `TERRANE_MLX_SERVER_BIN`.
    Env,
    /// `engines/mlx.json`, written by a previous `setup_mlx`.
    Manifest,
    /// Bare names found on PATH.
    Path,
}

impl RuntimeSource {
    pub fn describe(self) -> &'static str {
        match self {
            RuntimeSource::Env => "environment override",
            RuntimeSource::Manifest => "engines manifest",
            RuntimeSource::Path => "PATH",
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Manifest {
    generate_bin: String,
    server_bin: String,
    mlx_lm_version: Option<String>,
    installed_by: String,
}

pub fn engines_dir(home: &Path) -> PathBuf {
    home.join("engines")
}

fn manifest_path(home: &Path) -> PathBuf {
    engines_dir(home).join("mlx.json")
}

/// Find a usable MLX runtime without installing anything.
pub fn resolve_runtime(home: &Path) -> Option<MlxRuntime> {
    if let Some(generate_bin) = env_non_empty("TERRANE_MLX_LM_BIN") {
        let server_bin = env_non_empty("TERRANE_MLX_SERVER_BIN")
            .unwrap_or_else(|| sibling_server_bin(&generate_bin));
        return Some(MlxRuntime {
            generate_bin,
            server_bin,
            version: None,
            source: RuntimeSource::Env,
        });
    }
    if let Some(runtime) = manifest_runtime(home) {
        return Some(runtime);
    }
    if spawnable("mlx_lm.generate") {
        return Some(MlxRuntime {
            generate_bin: "mlx_lm.generate".into(),
            server_bin: "mlx_lm.server".into(),
            version: None,
            source: RuntimeSource::Path,
        });
    }
    None
}

fn manifest_runtime(home: &Path) -> Option<MlxRuntime> {
    let raw = fs::read_to_string(manifest_path(home)).ok()?;
    let manifest: Manifest = serde_json::from_str(&raw).ok()?;
    if !spawnable(&manifest.generate_bin) {
        return None;
    }
    Some(MlxRuntime {
        generate_bin: manifest.generate_bin,
        server_bin: manifest.server_bin,
        version: manifest.mlx_lm_version,
        source: RuntimeSource::Manifest,
    })
}

/// `…/mlx_lm.generate` → `…/mlx_lm.server`, preserving any directory prefix.
fn sibling_server_bin(generate_bin: &str) -> String {
    if let Some(prefix) = generate_bin.strip_suffix("mlx_lm.generate") {
        return format!("{prefix}mlx_lm.server");
    }
    "mlx_lm.server".to_string()
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn spawnable(bin: &str) -> bool {
    Command::new(bin)
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// The outcome of `setup_mlx`, for humans and UIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupReport {
    pub runtime: MlxRuntime,
    pub installed: bool,
    pub summary: String,
}

/// Ensure a usable MLX runtime exists, installing one when needed, and record
/// it in `engines/mlx.json`. Idempotent. `on_line` receives human-readable
/// progress lines (installer output, download progress).
pub fn setup_mlx(home: &Path, on_line: &mut dyn FnMut(&str)) -> Result<SetupReport, LlmError> {
    // A runtime that already resolves (env or PATH) just gets recorded so
    // later resolution no longer depends on ambient PATH.
    if let Some(found) = resolve_runtime(home) {
        write_manifest(home, &found, "detected")?;
        let summary = format!(
            "mlx runtime already available via {} ({}); recorded in engines/mlx.json",
            found.source.describe(),
            found.generate_bin
        );
        return Ok(SetupReport {
            runtime: found,
            installed: false,
            summary,
        });
    }

    let engines = engines_dir(home);
    let bin_dir = engines.join("bin");
    let uv = ensure_uv(&bin_dir, on_line)?;

    on_line(&format!("installing mlx-lm=={MLX_LM_VERSION} (this downloads Python and the MLX wheels on first run)"));
    let mut install = Command::new(&uv);
    install
        .args(["tool", "install", "--force"])
        .arg(format!("mlx-lm=={MLX_LM_VERSION}"))
        .arg("--with")
        .arg(format!("llguidance=={LLGUIDANCE_VERSION}"))
        .env("UV_TOOL_DIR", engines.join("uv-tools"))
        .env("UV_TOOL_BIN_DIR", &bin_dir)
        .env("UV_PYTHON_INSTALL_DIR", engines.join("python"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    let mut child = install
        .spawn()
        .map_err(|e| LlmError::Download(format!("failed to run uv: {e}")))?;
    // uv reports progress on stderr; forward it line by line.
    if let Some(stderr) = child.stderr.take() {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if !line.trim().is_empty() {
                on_line(&line);
            }
        }
    }
    let status = child
        .wait()
        .map_err(|e| LlmError::Download(format!("failed to wait for uv: {e}")))?;
    if !status.success() {
        return Err(LlmError::Download(format!(
            "uv tool install mlx-lm=={MLX_LM_VERSION} failed with {}",
            status.code().unwrap_or(-1)
        )));
    }

    let generate_bin = bin_dir.join("mlx_lm.generate");
    if !spawnable(&generate_bin.display().to_string()) {
        return Err(LlmError::Download(format!(
            "mlx-lm installed but {} is not runnable",
            generate_bin.display()
        )));
    }
    let runtime = MlxRuntime {
        generate_bin: generate_bin.display().to_string(),
        server_bin: bin_dir.join("mlx_lm.server").display().to_string(),
        version: Some(MLX_LM_VERSION.to_string()),
        source: RuntimeSource::Manifest,
    };
    write_manifest(home, &runtime, "uv")?;
    let summary = format!(
        "installed mlx-lm {MLX_LM_VERSION} under {} (self-contained)",
        engines.display()
    );
    Ok(SetupReport {
        runtime,
        installed: true,
        summary,
    })
}

/// Find `uv`, or bootstrap the pinned static binary into `bin_dir`.
fn ensure_uv(bin_dir: &Path, on_line: &mut dyn FnMut(&str)) -> Result<PathBuf, LlmError> {
    if spawnable("uv") {
        return Ok(PathBuf::from("uv"));
    }
    let local = bin_dir.join("uv");
    if spawnable(&local.display().to_string()) {
        return Ok(local);
    }

    let target = uv_release_target()?;
    let archive = format!("uv-{target}.tar.gz");
    let url = format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/{archive}");
    on_line(&format!("downloading uv {UV_VERSION} ({target})"));
    let (archive_path, _) = download_url(&url, bin_dir, &archive, &mut |_, _| {})?;

    // The archive holds `uv-<target>/uv`; strip the directory on extract.
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&archive_path)
        .arg("--strip-components=1")
        .arg("-C")
        .arg(bin_dir)
        .status()
        .map_err(|e| LlmError::Download(format!("failed to run tar: {e}")))?;
    let _ = fs::remove_file(&archive_path);
    if !status.success() {
        return Err(LlmError::Download(format!(
            "extracting {archive} failed with {}",
            status.code().unwrap_or(-1)
        )));
    }
    if !spawnable(&local.display().to_string()) {
        return Err(LlmError::Download(format!(
            "downloaded uv is not runnable at {}",
            local.display()
        )));
    }
    Ok(local)
}

fn uv_release_target() -> Result<&'static str, LlmError> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Ok("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Ok("x86_64-apple-darwin")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok("x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Ok("aarch64-unknown-linux-gnu")
    } else {
        Err(LlmError::Download(
            "no pinned uv build for this platform; install uv or mlx-lm manually".into(),
        ))
    }
}

fn write_manifest(home: &Path, runtime: &MlxRuntime, installed_by: &str) -> Result<(), LlmError> {
    let dir = engines_dir(home);
    fs::create_dir_all(&dir)
        .map_err(|e| LlmError::Download(format!("cannot create {}: {e}", dir.display())))?;
    let manifest = Manifest {
        generate_bin: runtime.generate_bin.clone(),
        server_bin: runtime.server_bin.clone(),
        mlx_lm_version: runtime.version.clone(),
        installed_by: installed_by.to_string(),
    };
    let raw = serde_json::to_string_pretty(&manifest)
        .map_err(|e| LlmError::Download(format!("manifest encode failed: {e}")))?;
    fs::write(manifest_path(home), raw)
        .map_err(|e| LlmError::Download(format!("manifest write failed: {e}")))
}
