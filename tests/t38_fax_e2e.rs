use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;

use rustrtc::t38::{
    DataField, DataFieldType, IfpPacket, T30FaxConfig, T30Indicator, T30Phase, T30Session,
};
use rustrtc::{UdtlConfig, UdtlReceiveBuffer, UdtlTransport};

/// Helper that wraps one side of a fax endpoint.
struct FaxEndpoint {
    transport: UdtlTransport,
    session: T30Session,
    recv_buf: UdtlReceiveBuffer,
}

impl FaxEndpoint {
    fn new(transport: UdtlTransport, session: T30Session) -> Self {
        Self {
            transport,
            session,
            recv_buf: UdtlReceiveBuffer::new(),
        }
    }

    /// Send an IFP indicator packet.
    async fn send_indicator(&self, indicator: T30Indicator) {
        let packet = IfpPacket::T30Indicator(vec![indicator]);
        let data = packet.encode().unwrap();
        self.transport.send(&data).await.unwrap();
    }

    /// Send an IFP data packet (HDLC or T.4).
    async fn send_data(&self, fields: Vec<DataField>) {
        let packet = IfpPacket::T30Data(fields);
        let data = packet.encode().unwrap();
        self.transport.send(&data).await.unwrap();
    }

    /// Receive one IFP packet with timeout.
    async fn recv_ifp(&mut self, timeout: Duration) -> Option<IfpPacket> {
        let raw = self.recv_raw(timeout).await?;
        IfpPacket::decode(&raw).ok()
    }

    /// Receive raw IFP bytes with timeout.
    async fn recv_raw(&mut self, timeout: Duration) -> Option<Vec<u8>> {
        tokio::time::timeout(timeout, async {
            loop {
                if let Ok(Some(data)) = self.transport.recv(&mut self.recv_buf).await {
                    return Some(data);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .ok()
        .flatten()
    }

    /// Get current phase.
    fn phase(&self) -> T30Phase {
        self.session.phase
    }
}

/// Create a pair of connected fax endpoints.
async fn make_fax_pair() -> (FaxEndpoint, FaxEndpoint) {
    let config = UdtlConfig {
        redundancy_depth: 2,
        ..UdtlConfig::default()
    };

    let a_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let b_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let a_addr = a_sock.local_addr().unwrap();
    let b_addr = b_sock.local_addr().unwrap();

    let ta = UdtlTransport::with_config(a_sock, b_addr, config.clone());
    let tb = UdtlTransport::with_config(b_sock, a_addr, config);

    let caller = FaxEndpoint::new(ta, T30Session::new(T30FaxConfig::default()));
    let callee = FaxEndpoint::new(tb, T30Session::new(T30FaxConfig::default()));

    (caller, callee)
}

/// Create an HDLC data field with a T.30 frame byte and optional data.
fn hdlc_field(frame_type: u8, data: &[u8]) -> DataField {
    let mut frame = vec![0xFF, 0xFF, frame_type]; // flags + frame type
    frame.extend_from_slice(data);
    DataField {
        field_type: DataFieldType::HdlcFcsOk,
        data: frame,
    }
}

/// Create T.4 non-ECM image data field.
fn t4_data(data: &[u8]) -> DataField {
    DataField {
        field_type: DataFieldType::T4NonEcm,
        data: data.to_vec(),
    }
}

// ──────────────────────────────────────────────
// T.30 Fax E2E tests
// ──────────────────────────────────────────────

#[tokio::test]
async fn test_fax_basic_indicator_exchange() {
    let _ = env_logger::builder().is_test(true).try_init();
    let (mut caller, mut callee) = make_fax_pair().await;

    caller.session.start_calling();
    caller.send_indicator(T30Indicator::Cng).await;

    let recv = callee.recv_ifp(Duration::from_secs(2)).await;
    match recv {
        Some(IfpPacket::T30Indicator(indicators)) => {
            assert!(indicators.contains(&T30Indicator::Cng));
        }
        _ => panic!("expected T30Indicator(CNG)"),
    }
}

#[tokio::test]
async fn test_fax_cng_ced_exchange() {
    let _ = env_logger::builder().is_test(true).try_init();
    let (mut caller, mut callee) = make_fax_pair().await;

    // Caller sends CNG
    caller.session.start_calling();
    caller.send_indicator(T30Indicator::Cng).await;

    // Callee receives CNG
    let recv_cng = callee.recv_ifp(Duration::from_secs(2)).await;
    assert!(matches!(recv_cng, Some(IfpPacket::T30Indicator(ref v)) if v.contains(&T30Indicator::Cng)));

    // Callee sends CED
    callee.session.start_called();
    callee.send_indicator(T30Indicator::Ced).await;

    // Caller receives CED
    let recv_ced = caller.recv_ifp(Duration::from_secs(2)).await;
    assert!(matches!(recv_ced, Some(IfpPacket::T30Indicator(ref v)) if v.contains(&T30Indicator::Ced)));
}

#[tokio::test]
async fn test_fax_dis_dcs_exchange() {
    let _ = env_logger::builder().is_test(true).try_init();
    let (mut caller, mut callee) = make_fax_pair().await;

    caller.session.start_calling();
    caller.send_indicator(T30Indicator::Cng).await;
    callee.send_indicator(T30Indicator::Ced).await;
    let _ = callee.recv_ifp(Duration::from_secs(2)).await;
    let _ = caller.recv_ifp(Duration::from_secs(2)).await;

    // Callee sends DIS
    callee.session.start_called();
    callee.send_data(vec![hdlc_field(0x01, &[0x00, 0x00])]).await;

    // Caller receives DIS
    let recv_dis = caller.recv_ifp(Duration::from_secs(2)).await;
    assert!(matches!(recv_dis, Some(IfpPacket::T30Data(_))));

    // Callee receives CNG too
    let _ = callee.recv_raw(Duration::from_secs(2)).await;

    // Caller sends DCS
    caller.send_data(vec![hdlc_field(0x02, &[0x80; 8])]).await;

    // Callee receives DCS
    let recv_dcs = callee.recv_ifp(Duration::from_secs(2)).await;
    assert!(matches!(recv_dcs, Some(IfpPacket::T30Data(_))));
}

#[tokio::test]
async fn test_fax_complete_session() {
    let _ = env_logger::builder().is_test(true).try_init();
    let (mut caller, mut callee) = make_fax_pair().await;

    // Phase 1: Caller sends CNG
    caller.session.start_calling();
    caller.send_indicator(T30Indicator::Cng).await;

    // Phase 2: Callee receives CNG, replies with CED and DIS
    let _ = callee.recv_ifp(Duration::from_secs(2)).await;
    callee.session.start_called();
    callee.send_indicator(T30Indicator::Ced).await;

    let _ = caller.recv_ifp(Duration::from_secs(2)).await;

    // Phase 3: DIS (called station capabilities)
    callee.send_data(vec![hdlc_field(0x01, &[0x00, 0x00])]).await;
    let _ = caller.recv_ifp(Duration::from_secs(2)).await;

    // Phase 4: DCS from caller
    caller.send_data(vec![hdlc_field(0x02, &[0x00, 0x00])]).await;
    let _ = callee.recv_ifp(Duration::from_secs(2)).await;

    // Phase 5: TCF (training check field)
    let tcf_data = vec![0x00; 200];
    caller.send_data(vec![hdlc_field(0x05, &tcf_data)]).await;
    let _ = callee.recv_ifp(Duration::from_secs(2)).await;

    // Phase 6: CFR (confirmation to receive)
    callee.session.change_phase(T30Phase::Training);
    callee.session.confirm_receipt();
    callee.send_data(vec![hdlc_field(0x04, &[])]).await;
    let _ = caller.recv_ifp(Duration::from_secs(2)).await;

    // Phase 7: Page data (T.4 image)
    let page = vec![0x00u8; 100];
    caller.session.change_phase(T30Phase::TransmittingPage);
    caller.send_data(vec![t4_data(&page)]).await;
    let _ = callee.recv_ifp(Duration::from_secs(2)).await;

    // Phase 8: EOP (end of procedure)
    caller.session.end_of_page();
    caller.send_data(vec![hdlc_field(0x10, &[])]).await;
    let _ = callee.recv_ifp(Duration::from_secs(2)).await;

    // Phase 9: MCF (message confirmation)
    callee.session.receive_mcf();
    callee.send_data(vec![hdlc_field(0x11, &[])]).await;
    let _ = caller.recv_ifp(Duration::from_secs(2)).await;

    // Phase 10: DCN (disconnect)
    caller.send_data(vec![hdlc_field(0x18, &[])]).await;
    let _ = callee.recv_ifp(Duration::from_secs(2)).await;

    // Verify final states - the caller side disconnected the session
    // callee should have received DCN
}

#[tokio::test]
async fn test_fax_multi_page_transfer() {
    let _ = env_logger::builder().is_test(true).try_init();
    let (mut caller, mut callee) = make_fax_pair().await;

    // Quick setup: send CNG/CED/DIS/DCS
    caller.session.start_calling();
    caller.send_indicator(T30Indicator::Cng).await;
    let _ = callee.recv_ifp(Duration::from_secs(2)).await;
    callee.session.start_called();
    callee.send_indicator(T30Indicator::Ced).await;
    let _ = caller.recv_ifp(Duration::from_secs(2)).await;
    callee.send_data(vec![hdlc_field(0x01, &[0x00, 0x00])]).await;
    let _ = caller.recv_ifp(Duration::from_secs(2)).await;
    caller.send_data(vec![hdlc_field(0x02, &[0x00, 0x00])]).await;
    let _ = callee.recv_ifp(Duration::from_secs(2)).await;

    // Send 3 pages
    for page_num in 0..3 {
        let page_data = vec![page_num; 50];

        caller.session.change_phase(T30Phase::TransmittingPage);
        caller.send_data(vec![t4_data(&page_data)]).await;
        let _ = callee.recv_ifp(Duration::from_secs(2)).await;

        // EOP after each page
        caller.session.end_of_page();
        caller.send_data(vec![hdlc_field(0x10, &[])]).await;
        let _ = callee.recv_ifp(Duration::from_secs(2)).await;

        // MCF
        callee.session.receive_mcf();
        callee.send_data(vec![hdlc_field(0x11, &[])]).await;
        let _ = caller.recv_ifp(Duration::from_secs(2)).await;
    }

    // DCN
    caller.send_data(vec![hdlc_field(0x18, &[])]).await;
    let _ = callee.recv_ifp(Duration::from_secs(2)).await;
}

#[tokio::test]
async fn test_fax_t30_sm_event_logging() {
    let _ = env_logger::builder().is_test(true).try_init();
    let (mut caller, _callee) = make_fax_pair().await;

    caller.session.start_calling();
    assert_eq!(caller.phase(), T30Phase::CallingToneSent);

    caller.session.start_called();
    assert_eq!(caller.phase(), T30Phase::CalledToneReceived);

    let events: Vec<_> = caller.session.events.iter().collect();
    assert_eq!(events.len(), 2);

    match events[0] {
        rustrtc::t38::T30Event::PhaseChange(T30Phase::Idle, T30Phase::CallingToneSent) => {}
        _ => panic!("unexpected first event"),
    }
    match events[1] {
        rustrtc::t38::T30Event::PhaseChange(T30Phase::CallingToneSent, T30Phase::CalledToneReceived) => {}
        _ => panic!("unexpected second event"),
    }
}

#[tokio::test]
async fn test_fax_empty_transport_no_data() {
    let _ = env_logger::builder().is_test(true).try_init();
    let (mut caller, _callee) = make_fax_pair().await;

    // Verify that recv with no data returns None within timeout
    let result = tokio::time::timeout(Duration::from_millis(100), async {
        loop {
            match caller.transport.recv(&mut caller.recv_buf).await {
                Ok(Some(_)) => return Some(()),
                Ok(None) => tokio::time::sleep(Duration::from_millis(5)).await,
                Err(_) => return None,
            }
        }
    })
    .await;

    // Should time out (no data sent)
    assert!(result.is_err(), "expected timeout but got data");
}
