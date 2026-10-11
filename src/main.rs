use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use colored::Colorize;
use std::net::SocketAddr;
use std::time::Duration;
use udptap::{ChaosConfig, InspectMode, UdpTapProxy};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ChaosDirection {
    Both,
    C2t,
    T2c,
}

#[derive(Parser, Debug)]
#[command(
    name = "udptap",
    about = "Zero-daemon UDP tap & chaos proxy with duplex inspection, packet drop, latency, duplication, and corruption",
    version
)]
struct Args {
    /// Local UDP address and port to bind listener on (e.g. "127.0.0.1:5000" or ":5000")
    #[arg(short, long, default_value = "127.0.0.1:5000")]
    listen: String,

    /// Target upstream UDP address and port to forward to (e.g. "127.0.0.1:8000")
    #[arg(short = 't', long = "target", alias = "forward")]
    target: String,

    /// Packet drop rate between 0.0 and 1.0 (e.g. 0.1 = 10% packet loss)
    #[arg(long, default_value_t = 0.0)]
    drop: f64,

    /// Additional simulated network latency in milliseconds
    #[arg(long, default_value_t = 0)]
    latency: u64,

    /// Simulated latency jitter in milliseconds (+/- jitter)
    #[arg(long, default_value_t = 0)]
    jitter: u64,

    /// Packet duplication rate between 0.0 and 1.0 (e.g. 0.05 = 5% duplicates)
    #[arg(long, default_value_t = 0.0)]
    duplicate: f64,

    /// Packet corruption rate between 0.0 and 1.0 (e.g. 0.02 = 2% bit flips)
    #[arg(long, default_value_t = 0.0)]
    corrupt: f64,

    /// Number of bytes to corrupt when a packet is corrupted
    #[arg(long, default_value_t = 1)]
    corrupt_bytes: usize,

    /// Direction to apply chaos to: "both", "c2t" (client-to-target), or "t2c" (target-to-client)
    #[arg(long, value_enum, default_value_t = ChaosDirection::Both)]
    direction: ChaosDirection,

    /// Bandwidth throttling rate limit in bytes per second
    #[arg(long)]
    throttle: Option<u64>,

    /// Show full 16-byte hex dump for every inspected packet
    #[arg(long)]
    hex: bool,

    /// Show ASCII preview line for every inspected packet
    #[arg(long, alias = "text")]
    ascii: bool,

    /// Suppress packet inspection logs (run silently)
    #[arg(short, long)]
    quiet: bool,

    /// Disable ANSI color formatting
    #[arg(long)]
    no_color: bool,

    /// Session idle timeout in seconds before closing upstream socket
    #[arg(long, default_value_t = 60)]
    idle_timeout: u64,
}

fn resolve_addr(addr_str: &str) -> Result<SocketAddr> {
    let normalized = if addr_str.starts_with(':') {
        format!("0.0.0.0{}", addr_str)
    } else {
        addr_str.to_string()
    };

    use std::net::ToSocketAddrs;
    let mut addrs = normalized
        .to_socket_addrs()
        .with_context(|| format!("Failed to resolve address '{}'", addr_str))?;

    addrs
        .next()
        .context(format!("No IP addresses found for '{}'", addr_str))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.no_color {
        colored::control::set_override(false);
    }

    if args.drop < 0.0 || args.drop > 1.0 {
        bail!("--drop must be between 0.0 and 1.0");
    }
    if args.duplicate < 0.0 || args.duplicate > 1.0 {
        bail!("--duplicate must be between 0.0 and 1.0");
    }
    if args.corrupt < 0.0 || args.corrupt > 1.0 {
        bail!("--corrupt must be between 0.0 and 1.0");
    }

    let listen_addr = resolve_addr(&args.listen)?;
    let target_addr = resolve_addr(&args.target)?;

    let base_chaos = ChaosConfig {
        drop_rate: args.drop,
        latency_ms: args.latency,
        jitter_ms: args.jitter,
        duplicate_rate: args.duplicate,
        corrupt_rate: args.corrupt,
        corrupt_bytes: args.corrupt_bytes,
        throttle_bytes_per_sec: args.throttle,
    };

    let (chaos_c2t, chaos_t2c) = match args.direction {
        ChaosDirection::Both => (base_chaos.clone(), base_chaos),
        ChaosDirection::C2t => (base_chaos, ChaosConfig::default()),
        ChaosDirection::T2c => (ChaosConfig::default(), base_chaos),
    };

    let inspect_mode = if args.quiet {
        InspectMode::Quiet
    } else if args.hex {
        InspectMode::Hex
    } else if args.ascii {
        InspectMode::Ascii
    } else {
        InspectMode::Summary
    };

    eprintln!(
        "{} Tap initialized: {} ⇄ {}",
        "●".cyan(),
        listen_addr.to_string().bold(),
        target_addr.to_string().bold()
    );

    if args.drop > 0.0 {
        eprintln!("  {} drop rate: {:.1}%", "⚡".yellow(), args.drop * 100.0);
    }
    if args.latency > 0 || args.jitter > 0 {
        eprintln!(
            "  {} latency: {}ms (±{}ms jitter)",
            "⚡".yellow(),
            args.latency,
            args.jitter
        );
    }
    if args.duplicate > 0.0 {
        eprintln!(
            "  {} duplicate rate: {:.1}%",
            "⚡".yellow(),
            args.duplicate * 100.0
        );
    }
    if args.corrupt > 0.0 {
        eprintln!(
            "  {} corrupt rate: {:.1}% ({} bytes)",
            "⚡".yellow(),
            args.corrupt * 100.0,
            args.corrupt_bytes
        );
    }
    if let Some(rate) = args.throttle {
        eprintln!("  {} bandwidth throttle: {} B/s", "⚡".yellow(), rate);
    }

    let proxy = UdpTapProxy::new(
        listen_addr,
        target_addr,
        chaos_c2t,
        chaos_t2c,
        inspect_mode,
        !args.no_color,
        Duration::from_secs(args.idle_timeout),
    );

    tokio::select! {
        res = proxy.run() => {
            if let Err(e) = res {
                eprintln!("{}: {}", "Error".red().bold(), e);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            eprintln!("\n{}", "Stopping UDP tap...".dimmed());
            proxy.stop();
        }
    }

    Ok(())
}
