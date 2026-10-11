use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use udptap::{ChaosConfig, InspectMode, UdpTapProxy};

#[tokio::test]
async fn test_udptap_duplex_passthrough() {
    // 1. Spawn Mock UDP Echo Server
    let echo_server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_server.local_addr().unwrap();

    let echo_handle = tokio::spawn(async move {
        let mut buf = vec![0u8; 1024];
        let (n, from) = echo_server.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"HELLO_UDP");
        // Reply
        echo_server.send_to(b"HELLO_REPLY", from).await.unwrap();
    });

    // 2. Spawn UdpTapProxy
    let tap_listener = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let tap_addr = tap_listener.local_addr().unwrap();
    drop(tap_listener); // free port for proxy bind

    let proxy = UdpTapProxy::new(
        tap_addr,
        echo_addr,
        ChaosConfig::default(),
        ChaosConfig::default(),
        InspectMode::Quiet,
        false,
        Duration::from_secs(10),
    );

    let proxy_handle = tokio::spawn(async move {
        let _ = proxy.run().await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    // 3. Client sends to Tap and expects reply
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.send_to(b"HELLO_UDP", tap_addr).await.unwrap();

    let mut resp = vec![0u8; 1024];
    let (n, from) = tokio::time::timeout(Duration::from_secs(2), client.recv_from(&mut resp))
        .await
        .expect("Timeout awaiting reply from tap")
        .expect("Failed to recv reply");

    assert_eq!(&resp[..n], b"HELLO_REPLY");
    assert_eq!(from, tap_addr);

    echo_handle.await.unwrap();
    proxy_handle.abort();
}

#[tokio::test]
async fn test_udptap_chaos_drop() {
    // 1. Echo server
    let echo_server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_server.local_addr().unwrap();

    let echo_handle = tokio::spawn(async move {
        let mut buf = vec![0u8; 1024];
        // Expect nothing received
        let res = tokio::time::timeout(Duration::from_millis(300), echo_server.recv_from(&mut buf)).await;
        assert!(res.is_err(), "Target should NOT have received dropped packet");
    });

    // 2. Tap with 100% drop rate
    let tap_listener = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let tap_addr = tap_listener.local_addr().unwrap();
    drop(tap_listener);

    let mut drop_chaos = ChaosConfig::default();
    drop_chaos.drop_rate = 1.0;

    let proxy = UdpTapProxy::new(
        tap_addr,
        echo_addr,
        drop_chaos,
        ChaosConfig::default(),
        InspectMode::Quiet,
        false,
        Duration::from_secs(10),
    );

    let proxy_handle = tokio::spawn(async move {
        let _ = proxy.run().await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.send_to(b"DROP_ME", tap_addr).await.unwrap();

    echo_handle.await.unwrap();
    proxy_handle.abort();
}

#[tokio::test]
async fn test_udptap_chaos_latency() {
    let echo_server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_server.local_addr().unwrap();

    let echo_handle = tokio::spawn(async move {
        let mut buf = vec![0u8; 1024];
        let (n, from) = echo_server.recv_from(&mut buf).await.unwrap();
        echo_server.send_to(&buf[..n], from).await.unwrap();
    });

    let tap_listener = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let tap_addr = tap_listener.local_addr().unwrap();
    drop(tap_listener);

    let mut latency_chaos = ChaosConfig::default();
    latency_chaos.latency_ms = 100; // 100ms added delay

    let proxy = UdpTapProxy::new(
        tap_addr,
        echo_addr,
        latency_chaos,
        ChaosConfig::default(),
        InspectMode::Quiet,
        false,
        Duration::from_secs(10),
    );

    let proxy_handle = tokio::spawn(async move {
        let _ = proxy.run().await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let start = Instant::now();
    client.send_to(b"LATENCY_TEST", tap_addr).await.unwrap();

    let mut resp = vec![0u8; 1024];
    let (n, _) = tokio::time::timeout(Duration::from_secs(2), client.recv_from(&mut resp))
        .await
        .unwrap()
        .unwrap();

    let elapsed = start.elapsed();
    assert_eq!(&resp[..n], b"LATENCY_TEST");
    assert!(
        elapsed >= Duration::from_millis(90),
        "Expected latency >= 90ms, got {:?}",
        elapsed
    );

    echo_handle.await.unwrap();
    proxy_handle.abort();
}

#[tokio::test]
async fn test_udptap_chaos_duplication() {
    let target = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let target_addr = target.local_addr().unwrap();

    let target_handle = tokio::spawn(async move {
        let mut buf = vec![0u8; 1024];
        // Read 1st packet
        let (n1, _) = target.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n1], b"DUP_TEST");

        // Read 2nd packet (duplicate)
        let (n2, _) = tokio::time::timeout(Duration::from_millis(500), target.recv_from(&mut buf))
            .await
            .expect("Expected duplicate packet")
            .unwrap();
        assert_eq!(&buf[..n2], b"DUP_TEST");
    });

    let tap_listener = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let tap_addr = tap_listener.local_addr().unwrap();
    drop(tap_listener);

    let mut dup_chaos = ChaosConfig::default();
    dup_chaos.duplicate_rate = 1.0;

    let proxy = UdpTapProxy::new(
        tap_addr,
        target_addr,
        dup_chaos,
        ChaosConfig::default(),
        InspectMode::Quiet,
        false,
        Duration::from_secs(10),
    );

    let proxy_handle = tokio::spawn(async move {
        let _ = proxy.run().await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.send_to(b"DUP_TEST", tap_addr).await.unwrap();

    target_handle.await.unwrap();
    proxy_handle.abort();
}
