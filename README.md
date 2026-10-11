# udptap

A zero-daemon UDP tap and chaos proxy with duplex datagram inspection, packet drop, latency/jitter, duplication, corruption, and bandwidth throttling.

`udptap` is the UDP counterpart to `tcptap`. It runs entirely in userspace without requiring root privileges (`iptables` / `tc netem`), managing stateful reverse NAT session tables so duplex UDP protocols (DNS, StatsD, Syslog, CoAP, WebRTC/RTP, and game networking) can be tapped, inspected, and subjected to real-world network degradation.

---

## Status

**Status: Complete & Verified**
- Bidirectional userspace session tracking and ephemeral socket mapping verified against live mock UDP sockets.
- Chaos pipeline verified: 100% packet drop, simulated latency & jitter (>=90ms RTT verification), packet duplication, and bit-level corruption.
- Inspection verified: 16-byte hex dumps with ASCII sidebar and inline single-line summaries.
- *Deferred*: PCAP/Wireshark export file format is reserved for a future release.

---

## Features

- **Zero Root Privileges**: Runs entirely in userspace — no `sudo`, no kernel modules, no iptables mangling.
- **Stateful Duplex Forwarding**: Tracks client sessions and routes return datagrams back to the originating client socket.
- **Duplex Chaos Engine**:
  - **Packet Drop**: `--drop <0.0-1.0>` (e.g. `0.1` for 10% simulated packet loss)
  - **Latency & Jitter**: `--latency <ms>` with `--jitter <ms>` variance
  - **Packet Duplication**: `--duplicate <0.0-1.0>` (e.g. `0.05` for 5% duplicate packets)
  - **Bit Corruption**: `--corrupt <0.0-1.0>` with configurable `--corrupt-bytes <N>`
  - **Bandwidth Throttling**: `--throttle <bytes_per_sec>` token-bucket rate limiter
  - **Directional Targeting**: Apply chaos symmetrically (`--direction both`) or unidirectionally (`--direction c2t` or `t2c`)
- **Real-Time Inspection**:
  - Directional indicators: `Client ──▶ Target` vs `Target ◀── Client`
  - Inline timestamps and payload byte counters
  - Hex dump mode (`--hex`) with offsets and ASCII representation
  - Printable ASCII mode (`--ascii`) for text-based UDP protocols
- **Session Auto-Reaper**: Cleans up idle upstream sockets after configurable inactivity (`--idle-timeout`).

---

## Installation

```bash
cargo install --path .
```

---

## Usage

### 1. Basic Inspection Tap

Tap UDP traffic between a local client and a remote DNS/Syslog/StatsD server:
```bash
udptap -l 127.0.0.1:5353 -t 1.1.1.1:53
```

Display full hex dumps for every packet:
```bash
udptap -l 127.0.0.1:8125 -t 127.0.0.1:8126 --hex
```

Display printable ASCII text (ideal for StatsD, Syslog, or custom JSON-over-UDP):
```bash
udptap -l 127.0.0.1:5140 -t 127.0.0.1:514 --ascii
```

### 2. Network Chaos Simulation

Simulate 15% packet loss on game telemetry or IoT UDP streams:
```bash
udptap -l :7777 -t 127.0.0.1:7778 --drop 0.15
```

Simulate cellular or satellite network delay (120ms latency ± 30ms jitter):
```bash
udptap -l :9000 -t 127.0.0.1:9001 --latency 120 --jitter 30
```

Simulate dirty wireless links with packet duplication and bit corruption:
```bash
udptap -l :6000 -t 127.0.0.1:6001 --duplicate 0.05 --corrupt 0.02
```

Simulate degraded upstream only (client-to-target path degraded, return path normal):
```bash
udptap -l :5000 -t 127.0.0.1:8000 --latency 200 --direction c2t
```

Throttle bandwidth to 64 KB/s:
```bash
udptap -l :5000 -t 127.0.0.1:8000 --throttle 65536
```

---

## CLI Reference

| Option | Description | Default |
|---|---|---|
| `-l, --listen <ADDR>` | Local UDP bind address | `127.0.0.1:5000` |
| `-t, --target <ADDR>` | Upstream UDP target address | Required |
| `--drop <RATE>` | Packet drop probability (0.0 - 1.0) | `0.0` |
| `--latency <MS>` | Added delay in milliseconds | `0` |
| `--jitter <MS>` | Latency jitter variance in milliseconds | `0` |
| `--duplicate <RATE>` | Packet duplication probability (0.0 - 1.0) | `0.0` |
| `--corrupt <RATE>` | Bit corruption probability (0.0 - 1.0) | `0.0` |
| `--corrupt-bytes <N>` | Number of bytes to flip per corrupted packet | `1` |
| `--direction <DIR>` | Chaos direction: `both`, `c2t`, `t2c` | `both` |
| `--throttle <BPS>` | Bandwidth limit in bytes per second | None |
| `--hex` | Display full 16-byte hex dump | False |
| `--ascii` | Display ASCII preview string | False |
| `-q, --quiet` | Suppress packet logs (silent proxy) | False |
| `--no-color` | Disable ANSI terminal styling | Color enabled |
| `--idle-timeout <SEC>`| Idle session expiration timeout | `60` |

---

## License

MIT License. Copyright (c) 2026 Animesh Raj.
