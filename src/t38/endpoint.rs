use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;

use crate::errors::RtcResult;
use crate::t38::ifp::{DataField, IfpPacket, T30Indicator};
use crate::t38::t30::{T30Event, T30Session};
#[cfg(test)]
use crate::t38::t30::T30FaxConfig;
use crate::transports::udptl::{UdtlConfig, UdtlReceiveBuffer, UdtlTransport};

/// High-level fax endpoint that ties together T.30 session, IFP codec,
/// and UDPTL transport into one unified API.
///
/// Typical usage:
/// ```ignore
/// let fax = FaxEndpoint::new(socket, remote_addr, T30FaxConfig::default());
///
/// // Caller side
/// fax.start_calling().await?;
/// fax.send_indicator(T30Indicator::Cng).await?;
///
/// // Callee side
/// fax.start_called().await?;
/// fax.send_indicator(T30Indicator::Ced).await?;
/// fax.send_data(vec![hdlc_field(0x01, &[0x00, 0x00])]).await?; // DIS
/// ```
pub struct FaxEndpoint {
    pub transport: Arc<UdtlTransport>,
    pub session: tokio::sync::Mutex<T30Session>,
    recv_buf: tokio::sync::Mutex<UdtlReceiveBuffer>,
    #[allow(dead_code)]
    config: UdtlConfig,
}

impl FaxEndpoint {
    /// Create a new fax endpoint from an existing transport and session.
    pub fn new(transport: Arc<UdtlTransport>, session: T30Session) -> Self {
        Self {
            transport,
            session: tokio::sync::Mutex::new(session),
            recv_buf: tokio::sync::Mutex::new(UdtlReceiveBuffer::new()),
            config: UdtlConfig::default(),
        }
    }

    /// Create a fax endpoint with a bound UDP socket and remote address.
    pub async fn bind(
        local: SocketAddr,
        remote: SocketAddr,
        session: T30Session,
        config: UdtlConfig,
    ) -> RtcResult<Self> {
        let socket = Arc::new(
            UdpSocket::bind(local)
                .await
                .map_err(|e| crate::errors::RtcError::Transport(format!("bind: {e}")))?,
        );
        let transport = Arc::new(UdtlTransport::with_config(socket, remote, config.clone()));
        Ok(Self {
            transport,
            session: tokio::sync::Mutex::new(session),
            recv_buf: tokio::sync::Mutex::new(UdtlReceiveBuffer::new()),
            config,
        })
    }

    /// Create from a pre-bound socket. Useful when the PeerConnection
    /// has already bound the socket during SDP generation.
    pub fn from_socket(
        socket: Arc<UdpSocket>,
        remote_addr: SocketAddr,
        session: T30Session,
    ) -> Self {
        let transport = Arc::new(UdtlTransport::new(socket, remote_addr));
        Self {
            transport,
            session: tokio::sync::Mutex::new(session),
            recv_buf: tokio::sync::Mutex::new(UdtlReceiveBuffer::new()),
            config: UdtlConfig::default(),
        }
    }

    // ── T.30 convenience methods ─────────────────────────────────

    /// Start the fax call as the calling station (sends CNG tone).
    pub async fn start_calling(&self) {
        self.session.lock().await.start_calling();
    }

    /// Answer the fax call as the called station.
    pub async fn start_called(&self) {
        self.session.lock().await.start_called();
    }

    // ── Sending ───────────────────────────────────────────────────

    /// Encode and send a T.30 indicator via UDPTL.
    pub async fn send_indicator(&self, ind: T30Indicator) -> RtcResult<()> {
        let packet = IfpPacket::T30Indicator(vec![ind]);
        let data = packet.encode()?;
        self.transport.send(&data).await
    }

    /// Encode and send a T.30 indicator via UDPTL in spandsp-compatible format.
    pub async fn send_indicator_spandsp(&self, ind: T30Indicator) -> RtcResult<()> {
        let packet = IfpPacket::T30Indicator(vec![ind]);
        let data = packet.encode_spandsp()?;
        self.transport.send(&data).await
    }

    /// Encode and send T.30 data fields via UDPTL.
    pub async fn send_data(&self, fields: Vec<DataField>) -> RtcResult<()> {
        let packet = IfpPacket::T30Data(fields);
        let data = packet.encode()?;
        self.transport.send(&data).await
    }

    // ── Receiving ─────────────────────────────────────────────────

    /// Receive the next IFP packet with a timeout.
    /// Returns `None` if no packet arrives within the timeout.
    pub async fn recv_timeout(&self, timeout: std::time::Duration) -> Option<IfpPacket> {
        tokio::time::timeout(timeout, self.recv())
            .await
            .ok()
            .flatten()
    }

    /// Receive the next IFP packet. Blocks until data arrives.
    pub async fn recv(&self) -> Option<IfpPacket> {
        loop {
            let mut buf = self.recv_buf.lock().await;
            match self.transport.recv(&mut buf).await {
                Ok(Some(raw)) => {
                    drop(buf);
                    match IfpPacket::decode(&raw) {
                        Ok(pkt) => return Some(pkt),
                        Err(e) => {
                            tracing::warn!("FaxEndpoint: IFP decode error: {e}");
                            continue;
                        }
                    }
                }
                Ok(None) => {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    continue;
                }
                Err(e) => {
                    tracing::warn!("FaxEndpoint: UDPTL recv error: {e}");
                    return None;
                }
            }
        }
    }

    /// Receive raw IFP bytes.
    pub async fn recv_raw(&self) -> RtcResult<Option<Vec<u8>>> {
        let mut buf = self.recv_buf.lock().await;
        self.transport.recv(&mut buf).await
    }

    // ── Drain events ──────────────────────────────────────────────

    /// Drain T.30 session events.
    pub fn drain_events(&self) -> Vec<T30Event> {
        let mut session = self.session.blocking_lock();
        let events: Vec<_> = session.events.drain(..).collect();
        events
    }

    // ── Reset ─────────────────────────────────────────────────────

    /// Reset the session and receive buffer.
    pub async fn reset(&self) {
        self.session.lock().await.reset();
        self.recv_buf.lock().await.reset(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::t38::ifp::DataFieldType;

    fn hdlc_field(frame_type: u8, data: &[u8]) -> DataField {
        let mut frame = vec![0xFF, 0xFF, frame_type];
        frame.extend_from_slice(data);
        DataField {
            field_type: DataFieldType::HdlcFcsOk,
            data: frame,
        }
    }

    #[tokio::test]
    async fn test_fax_endpoint_create_and_send_recv() {
        let a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let a_addr = a.local_addr().unwrap();
        let b_addr = b.local_addr().unwrap();

        let ta = Arc::new(a);
        let tb = Arc::new(b);

        let fax_a = FaxEndpoint::from_socket(ta, b_addr, T30Session::new(T30FaxConfig::default()));
        let fax_b = FaxEndpoint::from_socket(tb, a_addr, T30Session::new(T30FaxConfig::default()));

        fax_a.send_indicator(T30Indicator::Cng).await.unwrap();
        fax_a.session.lock().await.start_calling();

        let recv = tokio::time::timeout(std::time::Duration::from_secs(1), fax_b.recv())
            .await
            .unwrap();
        assert!(matches!(recv, Some(IfpPacket::T30Indicator(ref v)) if v.contains(&T30Indicator::Cng)));
    }

    #[tokio::test]
    async fn test_fax_endpoint_indicator_roundtrip() {
        let a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let a_addr = a.local_addr().unwrap();
        let b_addr = b.local_addr().unwrap();

        let fax_a = FaxEndpoint::from_socket(
            Arc::new(a),
            b_addr,
            T30Session::new(T30FaxConfig::default()),
        );
        let fax_b = FaxEndpoint::from_socket(
            Arc::new(b),
            a_addr,
            T30Session::new(T30FaxConfig::default()),
        );

        for ind in &[T30Indicator::Cng, T30Indicator::Ced, T30Indicator::V21Preamble] {
            fax_a.send_indicator(*ind).await.unwrap();
            let recv = tokio::time::timeout(std::time::Duration::from_millis(500), fax_b.recv())
                .await
                .unwrap();
            assert!(matches!(recv, Some(IfpPacket::T30Indicator(ref v)) if v.contains(ind)));
        }
    }

    #[tokio::test]
    async fn test_fax_endpoint_data_roundtrip() {
        let a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let a_addr = a.local_addr().unwrap();
        let b_addr = b.local_addr().unwrap();

        let fax_a = FaxEndpoint::from_socket(
            Arc::new(a),
            b_addr,
            T30Session::new(T30FaxConfig::default()),
        );
        let fax_b = FaxEndpoint::from_socket(
            Arc::new(b),
            a_addr,
            T30Session::new(T30FaxConfig::default()),
        );

        let data = vec![0xFF, 0x01, 0x02, 0x80, 0x20];
        fax_a
            .send_data(vec![hdlc_field(0x01, &data)])
            .await
            .unwrap();

        let recv = tokio::time::timeout(std::time::Duration::from_secs(1), fax_b.recv())
            .await
            .unwrap();
        assert!(matches!(recv, Some(IfpPacket::T30Data(_))));
    }

    #[tokio::test]
    async fn test_fax_endpoint_recv_timeout() {
        let a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let a_addr = a.local_addr().unwrap();
        let b_addr = b.local_addr().unwrap();

        let fax_b = FaxEndpoint::from_socket(
            Arc::new(b),
            a_addr,
            T30Session::new(T30FaxConfig::default()),
        );

        // No data sent, should timeout
        let result = fax_b
            .recv_timeout(std::time::Duration::from_millis(50))
            .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_fax_endpoint_bind() {
        let session = T30Session::new(T30FaxConfig::default());
        let remote = "127.0.0.1:9999".parse().unwrap();
        let fax = FaxEndpoint::bind("127.0.0.1:0".parse().unwrap(), remote, session, UdtlConfig::default())
            .await
            .unwrap();
        assert_eq!(fax.transport.local_addr().unwrap().ip().to_string(), "127.0.0.1");
    }
}
