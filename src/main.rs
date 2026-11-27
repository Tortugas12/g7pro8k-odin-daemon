use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use g7pro8k_odin_daemon::config::{Config, Profile};
use g7pro8k_odin_daemon::daemon::{run_daemon_loop, DaemonCommand, SharedDaemonState};
use g7pro8k_odin_daemon::diagnostics::{
    capture_from_usb, format_capture_jsonl, replay_capture_file, run_doctor, PidFileGuard,
    DEFAULT_CAPTURE_SECONDS, MAX_CAPTURE_SECONDS,
};
use g7pro8k_odin_daemon::transport::nusb_backend::probe_usb_devices;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliProfile {
    Xbox360,
    Ds4,
}

impl From<CliProfile> for Profile {
    fn from(value: CliProfile) -> Self {
        match value {
            CliProfile::Xbox360 => Profile::Xbox360,
            CliProfile::Ds4 => Profile::Ds4,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "g7pro8k-odin-daemon",
    version,
    about = "GameSir G7 Pro 8K root daemon & companion web app for AYN Odin 3 Pro"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run the daemon (USB reader -> coalescer -> virtual uinput + local Web UI).
    Run {
        /// Path to config.json (falls back to built-in defaults if absent).
        #[arg(long, default_value = "/data/local/tmp/g7pro8k/config.json")]
        config: PathBuf,
        /// Override virtual gamepad profile (`xbox360` or `ds4`).
        #[arg(long, value_enum)]
        profile: Option<CliProfile>,
        /// Run with headless mock output instead of opening /dev/uinput.
        #[arg(long)]
        dry_run: bool,
        /// Override local Web UI bind address (must be loopback, e.g. 127.0.0.1:8787).
        #[arg(long)]
        web_addr: Option<String>,
        /// Disable the embedded companion Web UI server.
        #[arg(long)]
        no_web: bool,
        /// Enable the isolated opt-in gyro DSU stub.
        #[arg(long)]
        enable_gyro_dsu: bool,
    },
    /// Probe connected USB devices and inspect allowlisted GameSir interfaces/endpoints.
    Probe {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Capture raw XInput + HID 0x12 USB packets to a .jsonl file for Day-1 verification.
    Capture {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long, default_value_t = DEFAULT_CAPTURE_SECONDS)]
        duration: u64,
        #[arg(long, default_value = "g7pro8k-capture.jsonl")]
        output: PathBuf,
    },
    /// Replay a recorded .jsonl capture through decoders, coalescer, and output pipeline.
    Replay {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        realtime: bool,
        #[arg(long)]
        enable_gyro_dsu: bool,
    },
    /// Check root privileges, /dev/uinput, USB bus, config validity, and Odin 3 isolation.
    Doctor {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Validate a config.json file and print any field-level errors.
    ValidateConfig {
        #[arg(long)]
        config: PathBuf,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match dispatch(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            error!("{msg}");
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

async fn dispatch(cli: Cli) -> Result<(), String> {
    match cli.command {
        Commands::Run {
            config: config_path,
            profile,
            dry_run,
            web_addr,
            no_web,
            enable_gyro_dsu,
        } => {
            let mut cfg = load_or_default_config(&config_path)?;
            if let Some(p) = profile {
                cfg.profile = p.into();
            }
            if let Some(addr) = web_addr {
                cfg.web_ui.bind = addr;
            }
            if no_web {
                cfg.web_ui.enabled = false;
            }
            cfg.validate().map_err(|e| format!("{e}: {:?}", e.errors))?;

            let _pid_guard = if dry_run {
                None
            } else {
                Some(
                    PidFileGuard::acquire(Path::new(&cfg.pid_file))
                        .map_err(|e| format!("single-instance lock failed: {e}"))?,
                )
            };

            let (shared, cmd_rx) = SharedDaemonState::new(config_path, cfg.clone(), dry_run);
            let daemon_shared = shared.clone();
            let worker = tokio::task::spawn_blocking(move || {
                run_daemon_loop(daemon_shared, cmd_rx, dry_run, enable_gyro_dsu);
            });

            let (web_shutdown_tx, web_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let web_handle = if cfg.web_ui.enabled {
                let bind_addr: std::net::SocketAddr = cfg
                    .web_ui
                    .bind
                    .parse()
                    .map_err(|e| format!("invalid web_ui.bind '{}': {e}", cfg.web_ui.bind))?;
                let web_shared = shared.clone();
                Some(tokio::spawn(async move {
                    if let Err(err) =
                        g7pro8k_odin_daemon::web::serve_web_ui(bind_addr, web_shared, async move {
                            let _ = web_shutdown_rx.await;
                        })
                        .await
                    {
                        error!("{err}");
                    }
                }))
            } else {
                None
            };

            info!("daemon running (press Ctrl+C or send SIGTERM to exit cleanly)");
            wait_for_shutdown_signal().await;
            let _ = web_shutdown_tx.send(());
            shared.send_command(DaemonCommand::Shutdown);
            if let Some(handle) = web_handle {
                let _ = handle.await;
            }
            let _ = worker.await;
            Ok(())
        }
        Commands::Probe { config, json } => {
            let cfg = load_optional_config(config.as_deref())?;
            let report = probe_usb_devices(&cfg.extra_pids).map_err(|e| e.to_string())?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
                );
            } else {
                println!(
                    "USB Probe: {} allowlisted GameSir G7 Pro 8K device(s) found ({} total USB devices)",
                    report.gamesir_count,
                    report.devices.len()
                );
                if let Some(quirk) = &report.quirk_warning {
                    println!("WARNING: {}", quirk.message);
                }
                for dev in &report.devices {
                    println!(
                        "  [{}:{}] {}:{} {:?} - {} {}",
                        dev.bus_id,
                        dev.device_address,
                        dev.vid,
                        dev.pid,
                        dev.classification,
                        dev.manufacturer.as_deref().unwrap_or("-"),
                        dev.product.as_deref().unwrap_or("-")
                    );
                    for iface in &dev.interfaces {
                        println!(
                            "      if={} class={} subclass={} proto={} endpoints={}",
                            iface.interface_number,
                            iface.class,
                            iface.subclass,
                            iface.protocol,
                            iface.endpoints.len()
                        );
                    }
                }
            }
            Ok(())
        }
        Commands::Capture {
            config,
            duration,
            output,
        } => {
            let cfg = load_optional_config(config.as_deref())?;
            let clamped = duration.clamp(1, MAX_CAPTURE_SECONDS);
            info!(seconds = clamped, output = %output.display(), "starting Day-1 USB capture");
            let records = capture_from_usb(&cfg, clamped).map_err(|e| e.to_string())?;
            let jsonl = format_capture_jsonl(&records).map_err(|e| e.to_string())?;
            fs::write(&output, jsonl).map_err(|e| format!("failed to write capture: {e}"))?;
            println!("Captured {} packets to {}", records.len(), output.display());
            Ok(())
        }
        Commands::Replay {
            config,
            input,
            dry_run,
            realtime,
            enable_gyro_dsu,
        } => {
            let cfg = load_optional_config(config.as_deref())?;
            let summary = replay_capture_file(&input, &cfg, dry_run, realtime, enable_gyro_dsu)
                .map_err(|e| e.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?
            );
            Ok(())
        }
        Commands::Doctor { config, json } => {
            let cfg = load_optional_config(config.as_deref())?;
            let report = run_doctor(&cfg);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
                );
            } else {
                println!(
                    "g7pro8k-odin-daemon doctor: {}",
                    if report.healthy {
                        "HEALTHY"
                    } else {
                        "ISSUES DETECTED"
                    }
                );
                for check in &report.checks {
                    println!(
                        "  [{}] {}: {}",
                        if check.ok { "OK" } else { "WARN" },
                        check.name,
                        check.detail
                    );
                }
            }
            Ok(())
        }
        Commands::ValidateConfig { config } => {
            let cfg = Config::load_from_file(&config).map_err(|e| e.to_string())?;
            println!(
                "Config {} is valid (profile={:?}, emit_rate_hz={}, web={})",
                config.display(),
                cfg.profile,
                cfg.emit_rate_hz,
                cfg.web_ui.bind
            );
            Ok(())
        }
    }
}

fn load_optional_config(path: Option<&Path>) -> Result<Config, String> {
    match path {
        Some(p) => Config::load_from_file(p).map_err(|e| format!("{}: {e}", p.display())),
        None => Ok(Config::default()),
    }
}

fn load_or_default_config(path: &Path) -> Result<Config, String> {
    if path.exists() {
        Config::load_from_file(path).map_err(|e| format!("{}: {e}", path.display()))
    } else {
        info!(path = %path.display(), "config file not found; using built-in defaults");
        Ok(Config::default())
    }
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("register SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
