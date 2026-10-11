use anyhow::{Context, Result};
use colored::Colorize;
use rand::Rng;
use std::collections::HashMap;
use std::fmt::Write as FmtWrite;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    ClientToTarget,
    TargetToClient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectMode {
    Summary,
    Ascii,
    Hex,
    Quiet,
}

#[derive(Debug, Clone)]
pub struct ChaosConfig {
    pub drop_rate: f64,
    pub latency_ms: u64,
    pub jitter_ms: u64,
    pub duplicate_rate: f64,
    pub corrupt_rate: f64,
    pub corrupt_bytes: usize,
    pub throttle_bytes_per_sec: Option<u64>,
}

impl Default for ChaosConfig {
    fn default() -> Self {
        Self {
            drop_rate: 0.0,
            latency_ms: 0,
            jitter_ms: 0,
            duplicate_rate: 0.0,
            corrupt_rate: 0.0,
            corrupt_bytes: 1,
            throttle_bytes_per_sec: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChaosDecision {
    Drop,
    Forward {
        payload: Vec<u8>,
        delay_ms: u64,
        duplicate: bool,
        corrupted: bool,
    },
}

impl ChaosConfig {
    pub fn process(&self, payload: &[u8]) -> ChaosDecision {
        let mut rng = rand::thread_rng();

        // 1. Drop check
        if self.drop_rate > 0.0 && rng.gen::<f64>() < self.drop_rate {
            return ChaosDecision::Drop;
        }

        // 2. Corrupt check
        let mut data = payload.to_vec();
        let corrupted = self.corrupt_rate > 0.0 && rng.gen::<f64>() < self.corrupt_rate;
        if corrupted && !data.is_empty() {
            let num_bytes = self.corrupt_bytes.max(1).min(data.len());
            for _ in 0..num_bytes {
                let idx = rng.gen_range(0..data.len());
                let bit = 1u8 << rng.gen_range(0..8);
                data[idx] ^= bit;
            }
        }

        // 3. Duplicate check
        let duplicate = self.duplicate_rate > 0.0 && rng.gen::<f64>() < self.duplicate_rate;

        // 4. Delay and Jitter
        let delay_ms = if self.latency_ms > 0 || self.jitter_ms > 0 {
            let jitter = if self.jitter_ms > 0 {
                rng.gen_range(0..=(self.jitter_ms * 2)) as i64 - (self.jitter_ms as i64)
            } else {
                0
            };
            (self.latency_ms as i64 + jitter).max(0) as u64
        } else {
            0
        };

        ChaosDecision::Forward {
            payload: data,
            delay_ms,
            duplicate,
            corrupted,
        }
    }
}

pub struct TokenBucket {
    rate_bytes_per_sec: f64,
    capacity: f64,
    tokens: Mutex<(f64, Instant)>,
}

impl TokenBucket {
    pub fn new(rate_bytes_per_sec: u64) -> Self {
        let rate = rate_bytes_per_sec as f64;
        Self {
            rate_bytes_per_sec: rate,
            capacity: rate.max(65536.0),
            tokens: Mutex::new((rate, Instant::now())),
        }
    }

    pub async fn consume(&self, bytes: usize) {
        let needed = bytes as f64;
        loop {
            let mut state = self.tokens.lock().await;
            let (mut current_tokens, last_time) = *state;
            let now = Instant::now();
            let elapsed = now.duration_since(last_time).as_secs_f64();

            current_tokens = (current_tokens + elapsed * self.rate_bytes_per_sec).min(self.capacity);

            if current_tokens >= needed {
                state.0 = current_tokens - needed;
                state.1 = now;
                return;
            }

            let deficit = needed - current_tokens;
            let wait_secs = deficit / self.rate_bytes_per_sec;
            drop(state);
            tokio::time::sleep(Duration::from_secs_f64(wait_secs)).await;
        }
    }
}

pub fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn format_hex_dump(data: &[u8]) -> String {
    let mut out = String::new();
    for (i, chunk) in data.chunks(16).enumerate() {
        let offset = i * 16;
        let _ = write!(out, "  {:04x}  ", offset);

        for (j, &b) in chunk.iter().enumerate() {
            let _ = write!(out, "{:02x} ", b);
            if j == 7 {
                out.push(' ');
            }
        }

        if chunk.len() < 16 {
            let missing = 16 - chunk.len();
            for j in 0..missing {
                out.push_str("   ");
                if chunk.len() + j == 7 {
                    out.push(' ');
                }
            }
        }

        out.push_str(" |");
        for &b in chunk {
            if (32..=126).contains(&b) {
                out.push(b as char);
            } else {
                out.push('.');
            }
        }
        out.push_str("|\n");
    }
    out
}

pub fn format_ascii_preview(data: &[u8], max_len: usize) -> String {
    let truncated = data.len() > max_len;
    let slice = if truncated { &data[..max_len] } else { data };
    let mut s: String = slice
        .iter()
        .map(|&b| if (32..=126).contains(&b) { b as char } else { '.' })
        .collect();
    if truncated {
        s.push_str("...");
    }
    s
}

pub fn format_log_entry(
    dir: Direction,
    client: SocketAddr,
    target: SocketAddr,
    payload_len: usize,
    decision: &ChaosDecision,
    color: bool,
) -> String {
    let ts = chrono::Utc::now().format("%H:%M:%S%.3f").to_string();

    let (dir_str, src_str, dst_str) = match dir {
        Direction::ClientToTarget => {
            let arrow = if color { "──▶".cyan().to_string() } else { "──▶".to_string() };
            (arrow, client.to_string(), target.to_string())
        }
        Direction::TargetToClient => {
            let arrow = if color { "◀──".magenta().to_string() } else { "◀──".to_string() };
            (arrow, target.to_string(), client.to_string())
        }
    };

    let mut chaos_tags = Vec::new();
    match decision {
        ChaosDecision::Drop => {
            let tag = if color { "[DROP]".red().bold().to_string() } else { "[DROP]".to_string() };
            chaos_tags.push(tag);
        }
        ChaosDecision::Forward {
            delay_ms,
            duplicate,
            corrupted,
            ..
        } => {
            if *delay_ms > 0 {
                let tag = format!("[+{}ms]", delay_ms);
                chaos_tags.push(if color { tag.yellow().to_string() } else { tag });
            }
            if *duplicate {
                let tag = "[DUP]";
                chaos_tags.push(if color { tag.purple().to_string() } else { tag.to_string() });
            }
            if *corrupted {
                let tag = "[CORRUPT]";
                chaos_tags.push(if color { tag.red().to_string() } else { tag.to_string() });
            }
        }
    }

    let chaos_str = if !chaos_tags.is_empty() {
        format!(" {}", chaos_tags.join(" "))
    } else {
        String::new()
    };

    let len_str = format!("[{} B]", payload_len);
    let len_styled = if color { len_str.dimmed().to_string() } else { len_str };

    format!("[{}] {} {} {} {}{}", ts, src_str, dir_str, dst_str, len_styled, chaos_str)
}

struct SessionEntry {
    upstream_socket: Arc<UdpSocket>,
    last_active: Arc<AtomicI64>,
    task_handle: tokio::task::JoinHandle<()>,
}

pub struct UdpTapProxy {
    listen_addr: SocketAddr,
    target_addr: SocketAddr,
    chaos_c2t: Arc<ChaosConfig>,
    chaos_t2c: Arc<ChaosConfig>,
    inspect_mode: InspectMode,
    color: bool,
    throttle: Option<Arc<TokenBucket>>,
    idle_timeout: Duration,
    running: Arc<AtomicBool>,
}

impl UdpTapProxy {
    pub fn new(
        listen_addr: SocketAddr,
        target_addr: SocketAddr,
        chaos_c2t: ChaosConfig,
        chaos_t2c: ChaosConfig,
        inspect_mode: InspectMode,
        color: bool,
        idle_timeout: Duration,
    ) -> Self {
        let throttle = chaos_c2t
            .throttle_bytes_per_sec
            .or(chaos_t2c.throttle_bytes_per_sec)
            .map(|r| Arc::new(TokenBucket::new(r)));

        Self {
            listen_addr,
            target_addr,
            chaos_c2t: Arc::new(chaos_c2t),
            chaos_t2c: Arc::new(chaos_t2c),
            inspect_mode,
            color,
            throttle,
            idle_timeout,
            running: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    pub async fn run(&self) -> Result<()> {
        let main_socket = Arc::new(
            UdpSocket::bind(self.listen_addr)
                .await
                .with_context(|| format!("Failed to bind UDP listener on {}", self.listen_addr))?,
        );

        let sessions: Arc<Mutex<HashMap<SocketAddr, SessionEntry>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Background session janitor
        let sessions_janitor = sessions.clone();
        let running_janitor = self.running.clone();
        let idle_timeout_ms = self.idle_timeout.as_millis() as i64;

        let janitor_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            while running_janitor.load(Ordering::SeqCst) {
                interval.tick().await;
                let now = now_epoch_ms();
                let mut map = sessions_janitor.lock().await;
                map.retain(|_addr, session| {
                    let active = session.last_active.load(Ordering::Relaxed);
                    if now - active > idle_timeout_ms {
                        session.task_handle.abort();
                        false
                    } else {
                        true
                    }
                });
            }
        });

        let mut recv_buf = vec![0u8; 65536];

        while self.running.load(Ordering::SeqCst) {
            let (n, client_addr) = match tokio::time::timeout(
                Duration::from_millis(500),
                main_socket.recv_from(&mut recv_buf),
            )
            .await
            {
                Ok(Ok(res)) => res,
                Ok(Err(_)) => break,
                Err(_) => continue, // Timeout tick to re-check running flag
            };

            let payload = recv_buf[..n].to_vec();

            // Find or create session for client_addr
            let (upstream_socket, last_active) = {
                let mut map = sessions.lock().await;
                if let Some(entry) = map.get(&client_addr) {
                    entry.last_active.store(now_epoch_ms(), Ordering::Relaxed);
                    (entry.upstream_socket.clone(), entry.last_active.clone())
                } else {
                    let upstream = Arc::new(
                        UdpSocket::bind("0.0.0.0:0")
                            .await
                            .context("Failed to bind ephemeral upstream UDP socket")?,
                    );
                    let last_active = Arc::new(AtomicI64::new(now_epoch_ms()));

                    // Reverse receiver task: Target -> Client
                    let up_socket_recv = upstream.clone();
                    let main_socket_fwd = main_socket.clone();
                    let chaos_t2c = self.chaos_t2c.clone();
                    let inspect_mode = self.inspect_mode;
                    let color = self.color;
                    let throttle = self.throttle.clone();
                    let last_active_clone = last_active.clone();

                    let task_handle = tokio::spawn(async move {
                        let mut resp_buf = vec![0u8; 65536];
                        loop {
                            match up_socket_recv.recv_from(&mut resp_buf).await {
                                Ok((len, from_src)) => {
                                    last_active_clone.store(now_epoch_ms(), Ordering::Relaxed);
                                    let resp_data = resp_buf[..len].to_vec();

                                    let decision = chaos_t2c.process(&resp_data);

                                    if inspect_mode != InspectMode::Quiet {
                                        let line = format_log_entry(
                                            Direction::TargetToClient,
                                            client_addr,
                                            from_src,
                                            len,
                                            &decision,
                                            color,
                                        );
                                        eprintln!("{}", line);
                                        match inspect_mode {
                                            InspectMode::Hex => {
                                                eprint!("{}", format_hex_dump(&resp_data));
                                            }
                                            InspectMode::Ascii => {
                                                eprintln!(
                                                    "  \"{}\"",
                                                    format_ascii_preview(&resp_data, 120)
                                                );
                                            }
                                            _ => {}
                                        }
                                    }

                                    if let ChaosDecision::Forward {
                                        payload,
                                        delay_ms,
                                        duplicate,
                                        ..
                                    } = decision
                                    {
                                        if let Some(ref th) = throttle {
                                            th.consume(payload.len()).await;
                                        }
                                        let s = main_socket_fwd.clone();
                                        let p_main = payload.clone();
                                        if delay_ms > 0 {
                                            tokio::spawn(async move {
                                                tokio::time::sleep(Duration::from_millis(delay_ms))
                                                    .await;
                                                let _ = s.send_to(&p_main, client_addr).await;
                                            });
                                        } else {
                                            let _ = s.send_to(&p_main, client_addr).await;
                                        }

                                        if duplicate {
                                            let s_dup = main_socket_fwd.clone();
                                            let p_dup = payload.clone();
                                            tokio::spawn(async move {
                                                tokio::time::sleep(Duration::from_millis(
                                                    delay_ms + 10,
                                                ))
                                                .await;
                                                let _ = s_dup.send_to(&p_dup, client_addr).await;
                                            });
                                        }
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                    });

                    let entry = SessionEntry {
                        upstream_socket: upstream.clone(),
                        last_active: last_active.clone(),
                        task_handle,
                    };
                    map.insert(client_addr, entry);
                    (upstream, last_active)
                }
            };

            last_active.store(now_epoch_ms(), Ordering::Relaxed);

            // Forward Client -> Target through Chaos
            let decision = self.chaos_c2t.process(&payload);

            if self.inspect_mode != InspectMode::Quiet {
                let line = format_log_entry(
                    Direction::ClientToTarget,
                    client_addr,
                    self.target_addr,
                    n,
                    &decision,
                    self.color,
                );
                eprintln!("{}", line);
                match self.inspect_mode {
                    InspectMode::Hex => {
                        eprint!("{}", format_hex_dump(&payload));
                    }
                    InspectMode::Ascii => {
                        eprintln!("  \"{}\"", format_ascii_preview(&payload, 120));
                    }
                    _ => {}
                }
            }

            if let ChaosDecision::Forward {
                payload: send_data,
                delay_ms,
                duplicate,
                ..
            } = decision
            {
                if let Some(ref th) = self.throttle {
                    th.consume(send_data.len()).await;
                }
                let target = self.target_addr;
                let up = upstream_socket.clone();
                let p_main = send_data.clone();

                if delay_ms > 0 {
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        let _ = up.send_to(&p_main, target).await;
                    });
                } else {
                    let _ = up.send_to(&p_main, target).await;
                }

                if duplicate {
                    let up_dup = upstream_socket.clone();
                    let send_dup = send_data.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(delay_ms + 10)).await;
                        let _ = up_dup.send_to(&send_dup, target).await;
                    });
                }
            }
        }

        janitor_handle.abort();
        let map = sessions.lock().await;
        for entry in map.values() {
            entry.task_handle.abort();
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_hex_dump() {
        let data = b"Hello, World!\x00\x01\x02";
        let hex = format_hex_dump(data);
        assert!(hex.contains("Hello, World!"));
        assert!(hex.contains("48 65 6c 6c 6f"));
    }

    #[test]
    fn test_format_ascii_preview() {
        let data = b"hello\x00world";
        let ascii = format_ascii_preview(data, 20);
        assert_eq!(ascii, "hello.world");
    }

    #[test]
    fn test_chaos_drop() {
        let mut cfg = ChaosConfig::default();
        cfg.drop_rate = 1.0;
        let res = cfg.process(b"test");
        assert_eq!(res, ChaosDecision::Drop);
    }

    #[test]
    fn test_chaos_corruption() {
        let mut cfg = ChaosConfig::default();
        cfg.corrupt_rate = 1.0;
        cfg.corrupt_bytes = 1;
        let original = b"unmodified";
        let res = cfg.process(original);
        match res {
            ChaosDecision::Forward {
                payload, corrupted, ..
            } => {
                assert!(corrupted);
                assert_ne!(&payload, original);
                assert_eq!(payload.len(), original.len());
            }
            _ => panic!("Expected corrupted forward"),
        }
    }
}
