//! Zero-server LAN discovery over UDP.
//!
//! ## Windows multi-instance problem
//! On Linux `SO_REUSEPORT` lets several processes receive the same datagram.
//! Windows has no such guarantee: unicast to a shared port lands in exactly
//! one process, so two nodes on the same PC would never see each other.
//!
//! ## Forced workaround
//! Every node **exclusively** owns the first free loopback discovery slot in
//! `127.0.0.1:48600..=48609` (bound *without* `SO_REUSEADDR`, which makes the
//! bind failure meaningful), and heartbeats fan out to:
//!   * `255.255.255.255:48600`
//!   * `239.255.48.60:48600`
//!   * the directed `/24` broadcast of every non-loopback IPv4 interface
//!   * all ten loopback slots
//!
//! The broadcast socket is bound to `0.0.0.0:48600` with `SO_REUSEADDR` so
//! many processes can listen for LAN broadcasts simultaneously.

use crate::db::{now_ms, PeerAnnounce};
use crate::{NebulaNode, PROTOCOL};
use serde::Deserialize;
use serde_json::json;
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket as StdUdpSocket};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 48, 60);

#[derive(Debug, Deserialize)]
struct DiscoveryPacket {
    #[serde(default)]
    protocol: String,
    #[serde(default)]
    packet_type: String,
    #[serde(default)]
    peer_id: String,
    #[serde(default)]
    nickname: String,
    #[serde(default)]
    bio: String,
    #[serde(default)]
    avatar_color: String,
    #[serde(default)]
    p2p_port: u16,
    #[serde(default)]
    moments_head_clock: i64,
    /// Sent by peers for observability; deliberately not used for staleness
    /// decisions because device clocks routinely disagree on a LAN.
    #[serde(default)]
    #[allow(dead_code)]
    timestamp: i64,
}

/// Start discovery tasks. Failures to bind individual sockets are logged but
/// do not abort the node: loopback fan-out and LAN broadcast are independent.
pub fn start(node: Arc<NebulaNode>) {
    let base = node.cfg.discovery_base_port;
    let slots = node.cfg.discovery_slots;

    match bind_sender() {
        Ok(std_socket) => {
            let socket = match tokio::net::UdpSocket::from_std(std_socket) {
                Ok(socket) => socket,
                Err(err) => {
                    warn!("discovery: cannot register sender socket: {err}");
                    return;
                }
            };
            tokio::spawn(heartbeat_loop(node.clone(), socket, base, slots));
        }
        Err(err) => warn!("discovery: cannot bind sender socket: {err}"),
    }

    match bind_broadcast(base) {
        Ok(std_socket) => match tokio::net::UdpSocket::from_std(std_socket) {
            Ok(socket) => {
                tokio::spawn(receive_loop(node.clone(), socket, "lan"));
            }
            Err(err) => warn!("discovery: cannot register broadcast socket: {err}"),
        },
        Err(err) => warn!("discovery: cannot bind broadcast port {base}: {err}"),
    }

    match bind_loopback(base, slots) {
        Ok((std_socket, port)) => {
            info!("discovery: loopback slot 127.0.0.1:{port}");
            match tokio::net::UdpSocket::from_std(std_socket) {
                Ok(socket) => {
                    tokio::spawn(receive_loop(node.clone(), socket, "loopback"));
                }
                Err(err) => warn!("discovery: cannot register loopback socket: {err}"),
            }
        }
        Err(err) => warn!(
            "discovery: no free loopback slot in {}..={}: {err}",
            base,
            base + slots.saturating_sub(1)
        ),
    }

    tokio::spawn(sweep_loop(node));
}

fn new_socket() -> std::io::Result<Socket> {
    Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
}

fn bind_sender() -> std::io::Result<StdUdpSocket> {
    let socket = new_socket()?;
    socket.set_broadcast(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0).into())?;
    Ok(socket.into())
}

fn bind_broadcast(base: u16) -> std::io::Result<StdUdpSocket> {
    let socket = new_socket()?;
    socket.set_reuse_address(true)?;
    socket.set_broadcast(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, base).into())?;
    // Best effort: join the multicast group so multicast announcements are
    // received too. Broadcast already works if the join is rejected.
    if let Err(err) = socket.join_multicast_v4(&MULTICAST_GROUP, &Ipv4Addr::UNSPECIFIED) {
        debug!("discovery: multicast join skipped: {err}");
    }
    Ok(socket.into())
}

/// Bind the first *exclusively available* loopback slot. `SO_REUSEADDR` is
/// deliberately not set: on Windows a successful bind would otherwise also
/// succeed for a slot owned by another process.
fn bind_loopback(base: u16, slots: u16) -> std::io::Result<(StdUdpSocket, u16)> {
    let mut last_err = None;
    for offset in 0..slots.max(1) {
        let port = base.saturating_add(offset);
        let socket = new_socket()?;
        socket.set_nonblocking(true)?;
        match socket.bind(&SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into()) {
            Ok(()) => return Ok((socket.into(), port)),
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, "no slot")))
}

async fn heartbeat_loop(
    node: Arc<NebulaNode>,
    socket: tokio::net::UdpSocket,
    base: u16,
    slots: u16,
) {
    let mut rx = node.subscribe_shutdown();
    loop {
        send_packet(&node, &socket, "ANNOUNCE", base, slots).await;
        tokio::select! {
            _ = tokio::time::sleep(node.cfg.heartbeat) => {}
            _ = node.announce_kick.notified() => {}
            _ = wait_shutdown(&mut rx) => {
                send_packet(&node, &socket, "LEAVE", base, slots).await;
                break;
            }
        }
    }
}

async fn send_packet(
    node: &NebulaNode,
    socket: &tokio::net::UdpSocket,
    packet_type: &str,
    base: u16,
    slots: u16,
) {
    let identity = node.identity();
    let head = node
        .db
        .local_moments_head(&identity.peer_id)
        .unwrap_or(0);
    let payload = json!({
        "protocol": PROTOCOL,
        "packet_type": packet_type,
        "peer_id": identity.peer_id,
        "nickname": identity.nickname,
        "bio": identity.bio,
        "avatar_color": identity.avatar_color,
        "p2p_port": node.p2p_port,
        "moments_head_clock": head,
        "timestamp": now_ms(),
    })
    .to_string();
    for dest in destinations(base, slots) {
        // Fire and forget: virtual adapters commonly reject broadcasts.
        let _ = socket.send_to(payload.as_bytes(), dest).await;
    }
}

/// Build the full fan-out destination list.
fn destinations(base: u16, slots: u16) -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = Vec::with_capacity(slots as usize + 8);
    let mut seen: HashSet<SocketAddr> = HashSet::new();

    let mut push = |out: &mut Vec<SocketAddr>, addr: SocketAddr| {
        if seen.insert(addr) {
            out.push(addr);
        }
    };

    push(
        &mut out,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), base),
    );
    push(
        &mut out,
        SocketAddr::new(IpAddr::V4(MULTICAST_GROUP), base),
    );
    for offset in 0..slots.max(1) {
        push(
            &mut out,
            SocketAddr::new(
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                base.saturating_add(offset),
            ),
        );
    }

    // Directed broadcasts for every real/virtual IPv4 interface (pitfall #2).
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for iface in interfaces {
            if iface.is_loopback() || iface.is_link_local() {
                continue;
            }
            if let if_addrs::IfAddr::V4(v4) = &iface.addr {
                let ip = v4.ip.octets();
                if ip[0] == 0 {
                    continue;
                }
                let mask = u32::from(v4.netmask);
                let broadcast = u32::from(v4.ip) | !mask;
                let [a, b, c, d] = broadcast.to_be_bytes();
                push(
                    &mut out,
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(a, b, c, d)), base),
                );
            }
        }
    }

    // Fallback when interface enumeration yields nothing useful.
    if out.len() == slots as usize + 2 {
        if let Some(ip) = probe_local_ipv4() {
            let o = ip.octets();
            push(
                &mut out,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(o[0], o[1], o[2], 255)), base),
            );
        }
    }
    out
}

/// Determine the outbound local IPv4 without transmitting any packet.
fn probe_local_ipv4() -> Option<Ipv4Addr> {
    for target in ["8.8.8.8:80", "1.1.1.1:80", "114.114.114.114:80"] {
        if let Ok(socket) = StdUdpSocket::bind("0.0.0.0:0") {
            if socket.connect(target).is_ok() {
                if let Ok(local) = socket.local_addr() {
                    if let IpAddr::V4(ip) = local.ip() {
                        if !ip.is_loopback() {
                            return Some(ip);
                        }
                    }
                }
            }
        }
    }
    None
}

async fn receive_loop(node: Arc<NebulaNode>, socket: tokio::net::UdpSocket, label: &'static str) {
    let mut rx = node.subscribe_shutdown();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        tokio::select! {
            result = socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, src)) => {
                        match serde_json::from_slice::<DiscoveryPacket>(&buf[..len]) {
                            Ok(packet) => handle_packet(&node, packet, src).await,
                            Err(err) => debug!("discovery({label}): bad packet from {src}: {err}"),
                        }
                    }
                    Err(err) => {
                        // ICMP port-unreachable surfaces as an error on Windows.
                        debug!("discovery({label}) recv error: {err}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
            _ = wait_shutdown(&mut rx) => break,
        }
    }
}

async fn handle_packet(node: &Arc<NebulaNode>, packet: DiscoveryPacket, src: SocketAddr) {
    if packet.protocol != PROTOCOL || packet.peer_id.trim().is_empty() {
        return;
    }
    let me = node.peer_id();
    if packet.peer_id == me {
        return;
    }
    match packet.packet_type.as_str() {
        "ANNOUNCE" => {
            let announce = PeerAnnounce {
                peer_id: packet.peer_id.clone(),
                nickname: packet.nickname,
                bio: packet.bio,
                avatar_color: packet.avatar_color,
                p2p_port: packet.p2p_port,
                moments_head_clock: packet.moments_head_clock,
            };
            if announce.p2p_port == 0 {
                return;
            }
            let src_ip = src.ip().to_string();
            match node.db.touch_peer(&announce, &src_ip, now_ms()) {
                Ok(touch) => {
                    if touch.became_online {
                        if let Ok(Some(peer)) = node.db.get_peer(&announce.peer_id) {
                            info!(
                                "peer online: {} ({}:{})",
                                peer.nickname, peer.last_ip, peer.last_p2p_port
                            );
                            node.emit(json!({"type": "PEER_ONLINE", "peer": peer}));
                        }
                    }
                    let queued = node
                        .db
                        .queued_messages_for(&announce.peer_id, &me, 1)
                        .map(|v| !v.is_empty())
                        .unwrap_or(false);
                    if touch.became_online || touch.head_clock_advanced || queued {
                        node.sync_kick.notify_one();
                    }
                }
                Err(err) => warn!("discovery: failed to update peer {}: {err:#}", packet.peer_id),
            }
        }
        "LEAVE" => {
            if let Ok(true) = node.db.set_peer_offline(&packet.peer_id) {
                info!("peer left: {}", packet.peer_id);
                node.emit(json!({"type": "PEER_OFFLINE", "peer_id": packet.peer_id}));
            }
        }
        other => debug!("discovery: ignoring packet type {other}"),
    }
}

async fn sweep_loop(node: Arc<NebulaNode>) {
    let mut rx = node.subscribe_shutdown();
    let timeout_ms = node.cfg.offline_timeout.as_millis() as i64;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(node.cfg.sweep_interval) => {}
            _ = wait_shutdown(&mut rx) => break,
        }
        match node.db.sweep_stale_peers(now_ms(), timeout_ms) {
            Ok(ids) => {
                for peer_id in ids {
                    info!("peer timed out: {peer_id}");
                    node.emit(json!({"type": "PEER_OFFLINE", "peer_id": peer_id}));
                }
            }
            Err(err) => warn!("discovery sweep failed: {err:#}"),
        }
    }
}

async fn wait_shutdown(rx: &mut tokio::sync::watch::Receiver<bool>) {
    if *rx.borrow() {
        return;
    }
    let _ = rx.changed().await;
}

/// Utility used by tests: is this address one of our loopback slots?
pub fn is_loopback_slot(addr: SocketAddr, base: u16, slots: u16) -> bool {
    addr.ip().is_loopback() && addr.port() >= base && addr.port() < base.saturating_add(slots)
}
