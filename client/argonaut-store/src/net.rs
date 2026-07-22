//! Client-side half of the D4 wire protocols (`argonaut-storage-provider`'s
//! `protocol.rs` is the server side; message *types* are shared via
//! `argonaut-primitives-storage`, but the length-prefix framing helpers
//! themselves are small enough to duplicate here rather than have this CLI
//! depend on the daemon crate for a client-side operation).

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, PublicKey, RelayMode, SecretKey, TransportAddr};
use std::net::SocketAddr;

pub async fn write_msg(
    send: &mut iroh::endpoint::SendStream,
    bytes: &[u8],
) -> anyhow::Result<()> {
    send.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
    send.write_all(bytes).await?;
    Ok(())
}

pub async fn read_msg(
    recv: &mut iroh::endpoint::RecvStream,
    max_len: usize,
) -> anyhow::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    anyhow::ensure!(len <= max_len, "message length {len} exceeds max {max_len}");
    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf).await?;
    Ok(buf)
}

/// An ephemeral client-side iroh endpoint: no relay, no DNS discovery --
/// same reasoning as the daemon side (`storage-provider/src/main.rs`),
/// providers' addresses come from on-chain `relay_hint` instead.
pub async fn client_endpoint() -> anyhow::Result<Endpoint> {
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    Ok(endpoint)
}

/// Parse a provider's on-chain `iroh_node_id` + `relay_hint` (an "ip:port"
/// string, per the daemon's own registration code) into an iroh
/// `EndpointAddr`, ready to `endpoint.connect()` against once the caller has
/// also told the endpoint about this address (see
/// [`connect_to_provider`], which does both steps).
pub fn provider_endpoint_addr(iroh_node_id: &[u8; 32], relay_hint: &[u8]) -> anyhow::Result<EndpointAddr> {
    let id = PublicKey::try_from(iroh_node_id)?;
    let addr_str = std::str::from_utf8(relay_hint)?;
    let addr: SocketAddr = addr_str.parse()?;
    Ok(EndpointAddr { id, addrs: [TransportAddr::Ip(addr)].into_iter().collect() })
}

/// Connect to a provider given its on-chain address info, over `alpn`.
pub async fn connect_to_provider(
    endpoint: &Endpoint,
    iroh_node_id: &[u8; 32],
    relay_hint: &[u8],
    alpn: &[u8],
) -> anyhow::Result<iroh::endpoint::Connection> {
    let addr = provider_endpoint_addr(iroh_node_id, relay_hint)?;
    // `connect_addr` connects directly against the given `EndpointAddr`
    // without needing the address registered in an address-lookup service
    // first (unlike `connect`, which resolves by `EndpointId` alone).
    Ok(endpoint.connect(addr, alpn).await?)
}
