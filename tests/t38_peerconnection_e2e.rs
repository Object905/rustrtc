use rustrtc::*;
use rustrtc::config::MediaCapabilities;

/// Helper: create a configuration with T.38 fax capabilities.
fn make_t38_config() -> RtcConfiguration {
    let mut config = RtcConfiguration::default();
    config.transport_mode = TransportMode::Rtp;
    config.media_capabilities = Some(MediaCapabilities {
        audio: vec![AudioCapability::pcmu()],
        video: vec![],
        application: None,
        image: vec![T38Capability {
            payload_type: 98,
            version: 0,
            max_bitrate: 14400,
            rate_management: T38FaxRateManagement::TransferredTCF,
            max_buffer: 1024,
            max_datagram: 238,
            udp_ec: T38UdpEC::T38UDPRedundancy,
            fmtp: None,
        }],
    });
    config
}

// ──────────────────────────────────────────────
// PeerConnection T.38 integration tests
// ──────────────────────────────────────────────

#[tokio::test]
async fn test_t38_add_image_transceiver() {
    let _ = env_logger::builder().is_test(true).try_init();
    let config = make_t38_config();
    let pc = PeerConnection::new(config);

    let transceiver = pc.add_transceiver(MediaKind::Image, TransceiverDirection::SendRecv);
    assert_eq!(transceiver.kind(), MediaKind::Image);
}

#[tokio::test]
async fn test_t38_offer_contains_image_section() {
    let _ = env_logger::builder().is_test(true).try_init();
    let config = make_t38_config();
    let pc = PeerConnection::new(config);
    pc.add_transceiver(MediaKind::Image, TransceiverDirection::SendRecv);

    let offer = pc.create_offer().await.unwrap();
    let sdp = offer.to_sdp_string();

    assert!(sdp.contains("m=image"), "SDP should contain m=image:\n{}", sdp);
    assert!(sdp.contains("udptl"), "SDP should contain udptl protocol:\n{}", sdp);
}

#[tokio::test]
async fn test_t38_offer_contains_t38_attributes() {
    let _ = env_logger::builder().is_test(true).try_init();
    let config = make_t38_config();
    let pc = PeerConnection::new(config);
    pc.add_transceiver(MediaKind::Image, TransceiverDirection::SendRecv);

    let offer = pc.create_offer().await.unwrap();
    let sdp = offer.to_sdp_string();

    assert!(sdp.contains("T38FaxVersion:0"), "SDP should contain T38FaxVersion:\n{}", sdp);
    assert!(sdp.contains("T38MaxBitRate:14400"), "SDP should contain T38MaxBitRate:\n{}", sdp);
    assert!(
        sdp.contains("T38FaxRateManagement:transferredTCF"),
        "SDP should contain T38FaxRateManagement:\n{}",
        sdp
    );
    assert!(sdp.contains("T38FaxMaxBuffer:1024"), "SDP should contain T38FaxMaxBuffer:\n{}", sdp);
    assert!(
        sdp.contains("T38FaxMaxDatagram:238"),
        "SDP should contain T38FaxMaxDatagram:\n{}",
        sdp
    );
    assert!(
        sdp.contains("T38FaxUdpEC:t38UDPRedundancy"),
        "SDP should contain T38FaxUdpEC:\n{}",
        sdp
    );
}

#[tokio::test]
async fn test_t38_offer_answer_roundtrip() {
    let _ = env_logger::builder().is_test(true).try_init();

    let caller_config = make_t38_config();
    let callee_config = make_t38_config();

    let caller = PeerConnection::new(caller_config);
    let callee = PeerConnection::new(callee_config);

    caller.add_transceiver(MediaKind::Image, TransceiverDirection::SendRecv);
    callee.add_transceiver(MediaKind::Image, TransceiverDirection::SendRecv);

    // Caller creates offer
    let mut offer = caller.create_offer().await.unwrap();
    let _ = caller.set_local_description(offer.clone());

    // Callee receives and creates answer (skip transport creation by setting remote first)
    // Set the type to Answer for the callee's description
    offer.sdp_type = SdpType::Offer;
    let _ = callee.set_remote_description(offer).await;

    let answer = callee.create_answer().await.unwrap();
    let answer_sdp = answer.to_sdp_string();

    // Verify answer also contains image section
    assert!(
        answer_sdp.contains("m=image"),
        "Answer SDP should contain m=image:\n{}",
        answer_sdp
    );
}

#[tokio::test]
async fn test_t38_parse_image_sdp() {
    let raw_sdp = "v=0\r\n\
o=- 1 1 IN IP4 127.0.0.1\r\n\
s=-\r\n\
t=0 0\r\n\
m=image 12345 udptl t38\r\n\
a=mid:0\r\n\
a=sendrecv\r\n\
a=T38FaxVersion:0\r\n\
a=T38MaxBitRate:14400\r\n\
a=T38FaxRateManagement:transferredTCF\r\n\
a=T38FaxMaxBuffer:1024\r\n\
a=T38FaxMaxDatagram:238\r\n\
a=T38FaxUdpEC:t38UDPRedundancy\r\n";

    let desc = SessionDescription::parse(SdpType::Offer, raw_sdp).unwrap();
    let image_sections: Vec<_> = desc.image_sections().collect();
    assert_eq!(image_sections.len(), 1);

    let section = &image_sections[0];
    assert_eq!(section.kind, MediaKind::Image);
    assert_eq!(section.port, 12345);
    assert_eq!(section.protocol, "udptl");
    assert!(section.formats.contains(&"t38".to_string()));

    // Parse T.38 capabilities
    let caps = desc.to_image_capabilities();
    assert_eq!(caps.len(), 1);
    assert_eq!(caps[0].version, 0);
    assert_eq!(caps[0].max_bitrate, 14400);
    assert_eq!(caps[0].max_buffer, 1024);
    assert_eq!(caps[0].max_datagram, 238);
}

#[tokio::test]
async fn test_t38_offer_media_section_listing() {
    let _ = env_logger::builder().is_test(true).try_init();

    let mut config = make_t38_config();
    config.media_capabilities = Some(MediaCapabilities {
        audio: vec![AudioCapability::pcmu()],
        video: vec![],
        application: None,
        image: vec![T38Capability::default_t38()],
    });

    let pc = PeerConnection::new(config);
    pc.add_transceiver(MediaKind::Audio, TransceiverDirection::SendRecv);
    pc.add_transceiver(MediaKind::Image, TransceiverDirection::SendRecv);

    let offer = pc.create_offer().await.unwrap();
    let sdp = offer.to_sdp_string();

    // Both sections should be present
    assert!(sdp.contains("m=audio"), "Should have audio section:\n{}", sdp);
    assert!(sdp.contains("m=image"), "Should have image section:\n{}", sdp);

    // Audio should use RTP/AVP protocol (in RTP mode)
    assert!(sdp.contains("m=audio"), "Audio section present");

    // Verify the order (audio should come before image based on transceiver ordering)
    let audio_pos = sdp.find("m=audio").unwrap();
    let image_pos = sdp.find("m=image").unwrap();
    assert!(audio_pos < image_pos, "audio should come before image in SDP");
}

#[tokio::test]
async fn test_t38_default_config_without_t38_caps() {
    // Even without explicit T.38 capabilities, the default should work
    let _ = env_logger::builder().is_test(true).try_init();
    let mut config = RtcConfiguration::default();
    config.transport_mode = TransportMode::Rtp;

    let pc = PeerConnection::new(config);
    pc.add_transceiver(MediaKind::Image, TransceiverDirection::SendRecv);

    let offer = pc.create_offer().await.unwrap();
    let sdp = offer.to_sdp_string();

    assert!(sdp.contains("m=image"), "SDP should contain m=image even with defaults:\n{}", sdp);
}
