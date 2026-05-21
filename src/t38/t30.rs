use crate::errors::RtcResult;
use std::collections::VecDeque;

/// T.30 fax phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum T30Phase {
    Idle,
    CallingToneSent,
    CalledToneReceived,
    Premessage,
    Training,
    ReadyToTransmit,
    TransmittingPage,
    PostPage,
    ReadyToReceive,
    ReceivingPage,
    Disconnecting,
}

/// T.30 fax resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum T30Resolution {
    Standard,  // 1728 x 215 dpi (approx)
    Fine,      // 1728 x 215 dpi × 2
    SuperFine, // 1728 x 391 dpi × 2
}

/// T.30 HDLC frame type identifiers (T.30 Annex A).
///
/// Note: Some frames share the same byte value in the protocol,
/// distinguished by direction. We assign unique enum discriminants
/// by extending the byte value with a direction offset (bytes 0x01-0x1F
/// for regular frames, 0x40+ for control frames).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdlcFrameType {
    /// Digital Identification Signal (called station capabilities)
    Dis = 0x01,
    /// Digital Command Signal (transmitter capabilities)
    Dcs = 0x02,
    /// Called Subscriber Identification
    Csi = 0x03,
    /// Non-standard Facilities
    Nsf = 0x04,
    /// Confirmation to Receive (ready for page)
    Cfr = 0x05,
    /// Training Check Field identifier
    Tcf = 0x06,
    /// Transmitter Subscriber Identification
    Tsi = 0x07,
    /// Subaddress
    Sub = 0x08,
    /// Selective Polling
    Sep = 0x09,
    /// Password
    Pwd = 0x0A,
    /// Frame Number Information (ECM mode)
    Ftt = 0x0B,
    /// End of Procedure
    Eop = 0x10,
    /// Message Confirmation
    Mcf = 0x11,
    /// Message Polling
    Mps = 0x12,
    /// Procedure Interrupt / End of Message
    Eom = 0x13,
    /// Partial Page Signal
    Pps = 0x14,
    /// Partial Page Request
    Ppr = 0x15,
    /// End of Retransmission
    Eor = 0x16,
    /// End of Retransmission Message
    EorMsg = 0x17,
    /// Training Re-check
    Trc = 0x18,
    /// Disconnect
    Dcn = 0x19,
    /// Pause
    Pause = 0x1A,
    /// Continue
    Continue = 0x1B,
    /// Received line count (RTN)
    Rtn = 0x1C,
}

/// Configuration for a T.30 fax session.
#[derive(Debug, Clone)]
pub struct T30FaxConfig {
    pub max_bitrate: u32,
    pub resolutions: Vec<T30Resolution>,
    pub ecm_supported: bool,
    pub local_id: String,
}

impl Default for T30FaxConfig {
    fn default() -> Self {
        Self {
            max_bitrate: 14400,
            resolutions: vec![T30Resolution::Standard, T30Resolution::Fine],
            ecm_supported: true,
            local_id: String::new(),
        }
    }
}

/// T.30 fax session state machine.
#[derive(Debug, Clone)]
pub struct T30Session {
    pub phase: T30Phase,
    pub local_config: T30FaxConfig,
    pub remote_config: Option<T30FaxConfig>,
    pub page_number: u32,
    pub page_data: Vec<u8>,
    /// V.21 HDLC frame buffer (for reassembly)
    pub hdlc_buffer: Vec<u8>,
    /// Event log for debugging/testing
    pub events: VecDeque<T30Event>,
}

/// Events that can occur during T.30 session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum T30Event {
    PhaseChange(T30Phase, T30Phase),
    RemoteIdentification { id: String },
    LocalIdentification { id: String },
    PageTransferred { page: u32, size: usize },
    PageReceived { page: u32, size: usize },
    Disconnected,
    Error(String),
}

impl T30Session {
    /// Create a new T.30 session.
    pub fn new(local_config: T30FaxConfig) -> Self {
        Self {
            phase: T30Phase::Idle,
            local_config,
            remote_config: None,
            page_number: 0,
            page_data: Vec::new(),
            hdlc_buffer: Vec::new(),
            events: VecDeque::new(),
        }
    }

    /// Start a fax transmission as the calling station.
    pub fn start_calling(&mut self) {
        self.change_phase(T30Phase::CallingToneSent);
    }

    /// Answer a fax call as the called station.
    pub fn start_called(&mut self) {
        self.change_phase(T30Phase::CalledToneReceived);
    }

    /// Process a received DIS (Digital Identification Signal) frame data.
    /// The data should contain the HDLC frame payload without flags/FCS.
    pub fn receive_dis(&mut self, data: &[u8]) -> RtcResult<()> {
        if self.phase == T30Phase::CalledToneReceived || self.phase == T30Phase::Premessage {
            // Parse DIS data to extract remote capabilities
            if data.len() >= 2 {
                let _dis_bits = (data[0] as u16) | ((data[1] as u16) << 8);
                // Bit 7 indicates ECM support
                let ecm_supported = (data[0] & 0x80) != 0;
                // Bits 8-9 indicate resolution
                let _fine = (data[1] & 0x01) != 0;
                let _super_fine = (data[1] & 0x02) != 0;
                // Bits 10-12 indicate max bitrate
                let _max_rate_bits = (data[1] >> 3) & 0x07;

                self.remote_config = Some(T30FaxConfig {
                    max_bitrate: Self::dis_bitrate(data[1]),
                    resolutions: vec![T30Resolution::Standard],
                    ecm_supported,
                    local_id: String::new(),
                });

                self.change_phase(T30Phase::Premessage);
            }
        }
        Ok(())
    }

    /// Send DCS (Digital Command Signal) to begin transmission.
    /// Returns the DCS frame data (without flags/FCS).
    pub fn send_dcs(&mut self) -> Vec<u8> {
        let mut dcs = vec![0x00, 0x00];
        // Set ECM if supported by both sides
        let remote_supports_ecm = self
            .remote_config
            .as_ref()
            .map(|r| r.ecm_supported)
            .unwrap_or(false);
        if self.local_config.ecm_supported && remote_supports_ecm {
            dcs[0] |= 0x80; // Bit 7 = ECM
        }
        dcs
    }

    /// Confirm receipt (CFR), transitioning to TransmittingPage.
    pub fn confirm_receipt(&mut self) {
        if self.phase == T30Phase::Training {
            self.change_phase(T30Phase::ReadyToTransmit);
            let page_size = self.page_data.len();
            self.page_number += 1;
            self.events.push_back(T30Event::PageTransferred {
                page: self.page_number,
                size: page_size,
            });
        }
    }

    /// Signal end of page (EOP).
    pub fn end_of_page(&mut self) {
        if self.phase == T30Phase::TransmittingPage {
            self.change_phase(T30Phase::PostPage);
        }
    }

    /// Receive MCF (Message Confirmation).
    pub fn receive_mcf(&mut self) {
        if self.phase == T30Phase::PostPage {
            // In a multi-page scenario would go back to Premessage
            self.change_phase(T30Phase::Disconnecting);
        }
    }

    /// Send DCN (Disconnect).
    pub fn send_dcn(&mut self) {
        self.change_phase(T30Phase::Disconnecting);
        self.events.push_back(T30Event::Disconnected);
    }

    /// Receive page data (T.4 image data).
    pub fn receive_page_data(&mut self, data: &[u8]) {
        self.page_data.extend_from_slice(data);
    }

    /// Complete page reception.
    pub fn complete_page_reception(&mut self) {
        let size = self.page_data.len();
        self.page_number += 1;
        self.events.push_back(T30Event::PageReceived {
            page: self.page_number,
            size,
        });
        self.page_data.clear();
    }

    /// Reset the session to idle.
    pub fn reset(&mut self) {
        self.phase = T30Phase::Idle;
        self.remote_config = None;
        self.page_number = 0;
        self.page_data.clear();
        self.hdlc_buffer.clear();
    }

    pub fn change_phase(&mut self, new_phase: T30Phase) {
        if self.phase != new_phase {
            let old = self.phase;
            self.phase = new_phase;
            self.events.push_back(T30Event::PhaseChange(old, new_phase));
        }
    }

    fn dis_bitrate(data_byte: u8) -> u32 {
        match (data_byte >> 3) & 0x07 {
            0 => 2400,
            1 => 4800,
            2 => 9600,
            3 => 12000,
            4 => 14400,
            _ => 2400,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_t30_session_initial_state() {
        let session = T30Session::new(T30FaxConfig::default());
        assert_eq!(session.phase, T30Phase::Idle);
        assert_eq!(session.page_number, 0);
        assert!(session.page_data.is_empty());
    }

    #[test]
    fn test_t30_start_calling() {
        let mut session = T30Session::new(T30FaxConfig::default());
        session.start_calling();
        assert_eq!(session.phase, T30Phase::CallingToneSent);
    }

    #[test]
    fn test_t30_start_called() {
        let mut session = T30Session::new(T30FaxConfig::default());
        session.start_called();
        assert_eq!(session.phase, T30Phase::CalledToneReceived);
    }

    #[test]
    fn test_t30_receive_dis() {
        let config = T30FaxConfig {
            ecm_supported: true,
            ..T30FaxConfig::default()
        };
        let mut session = T30Session::new(config);
        session.start_called();

        // DIS frame with ECM bit set and 14400 max rate
        // BIT 7 (0x80) = ECM, BIT 8 = Fine
        // Bits 10-12 = 4 for 14400
        let dis_data = vec![0x80, 0x20 | 0x01]; // ECM + Fine, rate=4 (14400)

        session.receive_dis(&dis_data).unwrap();
        assert_eq!(session.phase, T30Phase::Premessage);
        assert!(session.remote_config.is_some());

        let remote = session.remote_config.as_ref().unwrap();
        assert!(remote.ecm_supported);
    }

    #[test]
    fn test_t30_send_dcs() {
        let mut session = T30Session::new(T30FaxConfig {
            ecm_supported: true,
            ..T30FaxConfig::default()
        });
        session.start_called();

        // Remote also supports ECM
        session.remote_config = Some(T30FaxConfig {
            ecm_supported: true,
            ..T30FaxConfig::default()
        });

        let dcs = session.send_dcs();
        assert_eq!(dcs.len(), 2);
        assert!(dcs[0] & 0x80 != 0); // ECM bit should be set
    }

    #[test]
    fn test_t30_confirm_receipt() {
        let mut session = T30Session::new(T30FaxConfig::default());
        session.page_data = vec![0x00; 100];
        session.change_phase(T30Phase::Training);

        session.confirm_receipt();
        assert_eq!(session.phase, T30Phase::ReadyToTransmit);
        assert_eq!(session.page_number, 1);

        let event = session.events.back().unwrap();
        match event {
            T30Event::PageTransferred { page, size } => {
                assert_eq!(*page, 1);
                assert_eq!(*size, 100);
            }
            _ => panic!("expected PageTransferred event"),
        }
    }

    #[test]
    fn test_t30_end_of_page_and_mcf() {
        let mut session = T30Session::new(T30FaxConfig::default());
        session.change_phase(T30Phase::TransmittingPage);

        session.end_of_page();
        assert_eq!(session.phase, T30Phase::PostPage);

        session.receive_mcf();
        assert_eq!(session.phase, T30Phase::Disconnecting);
    }

    #[test]
    fn test_t30_send_dcn() {
        let mut session = T30Session::new(T30FaxConfig::default());
        session.send_dcn();
        assert_eq!(session.phase, T30Phase::Disconnecting);

        let event = session.events.back().unwrap();
        assert_eq!(*event, T30Event::Disconnected);
    }

    #[test]
    fn test_t30_receive_page_data() {
        let mut session = T30Session::new(T30FaxConfig::default());
        assert!(session.page_data.is_empty());

        session.receive_page_data(&[0x00, 0x01, 0x02]);
        assert_eq!(session.page_data.len(), 3);

        session.complete_page_reception();
        assert_eq!(session.page_number, 1);
        assert!(session.page_data.is_empty());

        let event = session.events.back().unwrap();
        match event {
            T30Event::PageReceived { page, size } => {
                assert_eq!(*page, 1);
                assert_eq!(*size, 3);
            }
            _ => panic!("expected PageReceived event"),
        }
    }

    #[test]
    fn test_t30_full_session_flow() {
        let config = T30FaxConfig {
            ecm_supported: true,
            local_id: "TEST001".to_string(),
            ..T30FaxConfig::default()
        };
        let mut session = T30Session::new(config);

        // Calling side flow
        session.start_calling();
        assert_eq!(session.phase, T30Phase::CallingToneSent);

        // Wait for CED from called side - simulate phase change
        // In real impl, CNG/CED detection would trigger this
        // For now, move to premessage via DIS reception
        let mut called_session = T30Session::new(T30FaxConfig::default());
        called_session.start_called();

        // Called side sends DIS
        let dis_data = vec![0x00, 0x00]; // no ECM, standard resolution
        called_session.receive_dis(&dis_data).unwrap();
        assert_eq!(called_session.phase, T30Phase::Premessage);

        // Send DCS from called side
        let _dcs = called_session.send_dcs();
        called_session.change_phase(T30Phase::Training);

        // Confirm receipt
        called_session.confirm_receipt();
        assert_eq!(called_session.phase, T30Phase::ReadyToTransmit);

        // Transmit page
        called_session.change_phase(T30Phase::TransmittingPage);

        // End page
        called_session.end_of_page();
        assert_eq!(called_session.phase, T30Phase::PostPage);

        // Receive MCF
        called_session.receive_mcf();
        assert_eq!(called_session.phase, T30Phase::Disconnecting);

        // Disconnect
        called_session.send_dcn();
        assert_eq!(called_session.phase, T30Phase::Disconnecting);
    }

    #[test]
    fn test_t30_reset() {
        let mut session = T30Session::new(T30FaxConfig::default());
        session.start_calling();
        session.page_number = 5;
        session.page_data = vec![0x00; 100];
        session.remote_config = Some(T30FaxConfig::default());

        session.reset();
        assert_eq!(session.phase, T30Phase::Idle);
        assert_eq!(session.page_number, 0);
        assert!(session.page_data.is_empty());
        assert!(session.remote_config.is_none());
    }

    #[test]
    fn test_t30_events_logged_on_phase_transitions() {
        let mut session = T30Session::new(T30FaxConfig::default());

        // Phase transitions should generate events
        session.start_calling();
        session.start_called();

        let events: Vec<_> = session.events.iter().collect();
        assert_eq!(events.len(), 2);

        match events[0] {
            T30Event::PhaseChange(T30Phase::Idle, T30Phase::CallingToneSent) => {}
            _ => panic!("unexpected event"),
        }

        match events[1] {
            T30Event::PhaseChange(T30Phase::CallingToneSent, T30Phase::CalledToneReceived) => {}
            _ => panic!("unexpected event"),
        }
    }
}
