use crate::errors::{RtcError, RtcResult};
use crate::t38::per::{BitReader, BitWriter, PerCodec};

/// T.30 Indicator types (T.38 Annex A / ITU-T Recommendation T.38 §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum T30Indicator {
    NoSignal = 0,
    Cng = 1,
    Ced = 2,
    V21Preamble = 3,
    V27Ter2400Preamble = 4,
    V27Ter4800Preamble = 5,
    V297200Preamble = 6,
    V299600Preamble = 7,
    V177200ShortPreamble = 8,
    V177200LongPreamble = 9,
    V179600ShortPreamble = 10,
    V179600LongPreamble = 11,
    V1712000ShortPreamble = 12,
    V1712000LongPreamble = 13,
    V1714400ShortPreamble = 14,
    V1714400LongPreamble = 15,
    V8AnsAm = 16,
    V8Signal = 17,
    V34Preamble = 18,
    V34ControlChannel = 19,
}

impl T30Indicator {
    /// Maximum known indicator value (for range checking).
    const MAX_VAL: u8 = 19;

    /// Create from integer value.
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::NoSignal),
            1 => Some(Self::Cng),
            2 => Some(Self::Ced),
            3 => Some(Self::V21Preamble),
            4 => Some(Self::V27Ter2400Preamble),
            5 => Some(Self::V27Ter4800Preamble),
            6 => Some(Self::V297200Preamble),
            7 => Some(Self::V299600Preamble),
            8 => Some(Self::V177200ShortPreamble),
            9 => Some(Self::V177200LongPreamble),
            10 => Some(Self::V179600ShortPreamble),
            11 => Some(Self::V179600LongPreamble),
            12 => Some(Self::V1712000ShortPreamble),
            13 => Some(Self::V1712000LongPreamble),
            14 => Some(Self::V1714400ShortPreamble),
            15 => Some(Self::V1714400LongPreamble),
            16 => Some(Self::V8AnsAm),
            17 => Some(Self::V8Signal),
            18 => Some(Self::V34Preamble),
            19 => Some(Self::V34ControlChannel),
            _ => None,
        }
    }
}

/// Field type for T.30 data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataFieldType {
    HdlcData = 0,
    HdlcSigEnd = 1,
    HdlcFcsOk = 2,
    HdlcFcsBad = 3,
    HdlcFcsOkSigEnd = 4,
    HdlcFcsBadSigEnd = 5,
    T4NonEcm = 6,
}

impl DataFieldType {
    const MAX_VAL: u8 = 6;

    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::HdlcData),
            1 => Some(Self::HdlcSigEnd),
            2 => Some(Self::HdlcFcsOk),
            3 => Some(Self::HdlcFcsBad),
            4 => Some(Self::HdlcFcsOkSigEnd),
            5 => Some(Self::HdlcFcsBadSigEnd),
            6 => Some(Self::T4NonEcm),
            _ => None,
        }
    }
}

/// A single data field in an IFP t30-data packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataField {
    pub field_type: DataFieldType,
    pub data: Vec<u8>,
}

/// IFP packet types (T.38 §7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IfpPacket {
    /// Contains T.30 indicators (signals).
    T30Indicator(Vec<T30Indicator>),
    /// Contains T.30 data (HDLC frames or T.4 image data).
    T30Data(Vec<DataField>),
}

impl IfpPacket {
    /// Encode this IFP packet into bytes using ASN.1 PER.
    pub fn encode(&self) -> RtcResult<Vec<u8>> {
        let mut buf = BitWriter::new();

        // type-of-msg (0..1) -> 1 bit
        let type_of_msg: u8 = match self {
            Self::T30Indicator(_) => 0,
            Self::T30Data(_) => 1,
        };
        PerCodec::encode_int(type_of_msg as u64, 0, 1, &mut buf)?;

        match self {
            Self::T30Indicator(indicators) => {
                // choice index 0 for t30-indicator
                PerCodec::encode_choice_index(0, 2, &mut buf)?;
                // SEQUENCE OF T30Indicator
                PerCodec::encode_length(indicators.len(), Some(31), &mut buf)?;
                for ind in indicators {
                    PerCodec::encode_small_int(*ind as u8, 0, T30Indicator::MAX_VAL, &mut buf);
                }
            }
            Self::T30Data(fields) => {
                // choice index 1 for t30-data
                PerCodec::encode_choice_index(1, 2, &mut buf)?;
                // SEQUENCE OF DataField
                PerCodec::encode_length(fields.len(), Some(31), &mut buf)?;
                for field in fields {
                    PerCodec::encode_small_int(
                        field.field_type as u8,
                        0,
                        DataFieldType::MAX_VAL,
                        &mut buf,
                    );
                    PerCodec::encode_octet_string(&field.data, 65535, &mut buf)?;
                }
            }
        }

        Ok(buf.into_bytes())
    }

    /// Encode this IFP packet in spandsp-compatible (simplified) format.
    ///
    /// spandsp (FreeSWITCH/Asterisk) uses a non-standard lightweight binary format
    /// instead of T.38 Annex A ASN.1 PER. This format is NOT interoperable with
    /// standard T.38 — use only when talking to spandsp-based endpoints.
    ///
    /// Format (indicators, 1 byte):
    ///   byte[0] = (type_of_msg << 7) | (indicator_value << 1)
    ///   type_of_msg: 0 = t30-indicator
    ///   indicator_value: T.38 indicator value (0-22)
    ///
    /// Format (data, variable):
    ///   byte[0] = 0x80 | (field_count << 1)  // type=1(data), count in bits 6-2
    ///   followed by field entries per spandsp's internal format
    ///
    /// Note: For data packets, spandsp uses a complex internal format that we
    /// don't fully replicate here. Use `encode()` for standard T.38 PER encoding.
    pub fn encode_spandsp(&self) -> RtcResult<Vec<u8>> {
        match self {
            Self::T30Indicator(indicators) => {
                if indicators.len() > 1 {
                    return Err(RtcError::Protocol(
                        "spandsp format only supports single indicators".into(),
                    ));
                }
                let val = if let Some(ind) = indicators.first() {
                    *ind as u8
                } else {
                    0
                };
                // type_of_msg=0 (bit 7=0), value << 1 (bits 6-2), padding (bits 1-0)
                Ok(vec![val << 1])
            }
            Self::T30Data(_fields) => {
                // spandsp's data format is internal/opaque. Use encode() for
                // standard T.38 PER that other T.38 implementations can decode.
                Err(RtcError::NotImplemented(
                    "spandsp data encoding not implemented — use encode() for standard T.38 PER",
                ))
            }
        }
    }

    /// Decode an IFP packet from bytes.
    pub fn decode(data: &[u8]) -> RtcResult<Self> {
        let mut buf = BitReader::new(data.to_vec());

        let type_of_msg = PerCodec::decode_int(&mut buf, 0, 1)?;

        match type_of_msg {
            0 => {
                let _choice = PerCodec::decode_choice_index(&mut buf, 2)?;
                let count = PerCodec::decode_length(&mut buf, Some(31))?;
                let mut indicators = Vec::with_capacity(count);
                for _ in 0..count {
                    let val = PerCodec::decode_small_int(&mut buf, 0, T30Indicator::MAX_VAL);
                    indicators.push(T30Indicator::from_u8(val).ok_or_else(|| {
                        RtcError::Protocol(format!("invalid T30Indicator: {}", val))
                    })?);
                }
                Ok(Self::T30Indicator(indicators))
            }
            1 => {
                let _choice = PerCodec::decode_choice_index(&mut buf, 2)?;
                let count = PerCodec::decode_length(&mut buf, Some(31))?;
                let mut fields = Vec::with_capacity(count);
                for _ in 0..count {
                    let ft = PerCodec::decode_small_int(&mut buf, 0, DataFieldType::MAX_VAL);
                    let field_type = DataFieldType::from_u8(ft).ok_or_else(|| {
                        RtcError::Protocol(format!("invalid DataFieldType: {}", ft))
                    })?;
                    let data = PerCodec::decode_octet_string(&mut buf, 65535)?;
                    fields.push(DataField { field_type, data });
                }
                Ok(Self::T30Data(fields))
            }
            _ => Err(RtcError::Protocol(format!(
                "unknown IFP packet type: {}",
                type_of_msg
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_t30_indicator() {
        let packet = IfpPacket::T30Indicator(vec![
            T30Indicator::Cng,
            T30Indicator::Ced,
            T30Indicator::V21Preamble,
        ]);

        let encoded = packet.encode().unwrap();
        let decoded = IfpPacket::decode(&encoded).unwrap();

        assert_eq!(packet, decoded);
    }

    #[test]
    fn test_encode_decode_t30_data() {
        let packet = IfpPacket::T30Data(vec![
            DataField {
                field_type: DataFieldType::HdlcData,
                data: vec![0xFF, 0x01, 0x02],
            },
            DataField {
                field_type: DataFieldType::T4NonEcm,
                data: vec![0x00; 64],
            },
        ]);

        let encoded = packet.encode().unwrap();
        let decoded = IfpPacket::decode(&encoded).unwrap();

        assert_eq!(packet, decoded);
    }

    #[test]
    fn test_encode_decode_empty_indicator() {
        let packet = IfpPacket::T30Indicator(vec![]);
        let encoded = packet.encode().unwrap();
        let decoded = IfpPacket::decode(&encoded).unwrap();
        assert_eq!(packet, decoded);
    }

    #[test]
    fn test_encode_decode_empty_data() {
        let packet = IfpPacket::T30Data(vec![]);
        let encoded = packet.encode().unwrap();
        let decoded = IfpPacket::decode(&encoded).unwrap();
        assert_eq!(packet, decoded);
    }

    #[test]
    fn test_encode_decode_single_indicator() {
        for ind in 0..=19 {
            let indicator = T30Indicator::from_u8(ind).unwrap();
            let packet = IfpPacket::T30Indicator(vec![indicator]);
            let encoded = packet.encode().unwrap();
            let decoded = IfpPacket::decode(&encoded).unwrap();
            assert_eq!(packet, decoded, "failed for indicator {}", ind);
        }
    }

    #[test]
    fn test_decode_invalid_data() {
        let result = IfpPacket::decode(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_t30_indicator_roundtrip_all_variants() {
        let all: Vec<T30Indicator> = (0..=19)
            .map(|i| T30Indicator::from_u8(i).unwrap())
            .collect();
        let packet = IfpPacket::T30Indicator(all);
        let encoded = packet.encode().unwrap();
        let decoded = IfpPacket::decode(&encoded).unwrap();
        assert_eq!(packet, decoded);
    }

    #[test]
    fn test_data_field_types_all() {
        let mut fields = Vec::new();
        for ft in 0..=6 {
            let field_type = DataFieldType::from_u8(ft).unwrap();
            fields.push(DataField {
                field_type,
                data: vec![ft as u8; ft as usize + 1],
            });
        }
        let packet = IfpPacket::T30Data(fields);
        let encoded = packet.encode().unwrap();
        let decoded = IfpPacket::decode(&encoded).unwrap();
        assert_eq!(packet, decoded);
    }

    #[test]
    fn test_large_data_field() {
        let data = (0..255).collect::<Vec<_>>();
        let packet = IfpPacket::T30Data(vec![DataField {
            field_type: DataFieldType::HdlcFcsOk,
            data,
        }]);
        let encoded = packet.encode().unwrap();
        let decoded = IfpPacket::decode(&encoded).unwrap();
        assert_eq!(packet, decoded);
    }

    #[test]
    fn test_t30_indicator_unknown() {
        assert!(T30Indicator::from_u8(20).is_none());
        assert!(T30Indicator::from_u8(255).is_none());
    }

    #[test]
    fn test_data_field_type_unknown() {
        assert!(DataFieldType::from_u8(7).is_none());
        assert!(DataFieldType::from_u8(255).is_none());
    }

    #[test]
    fn test_encode_spandsp_cng() {
        let packet = IfpPacket::T30Indicator(vec![T30Indicator::Cng]);
        let encoded = packet.encode_spandsp().unwrap();
        assert_eq!(encoded, vec![0x02]);
    }

    #[test]
    fn test_encode_spandsp_v21() {
        let packet = IfpPacket::T30Indicator(vec![T30Indicator::V21Preamble]);
        let encoded = packet.encode_spandsp().unwrap();
        assert_eq!(encoded, vec![0x06]);
    }

    #[test]
    fn test_encode_spandsp_all_indicators() {
        for val in 0..=19 {
            let Some(ind) = T30Indicator::from_u8(val) else {
                continue;
            };
            let packet = IfpPacket::T30Indicator(vec![ind]);
            let encoded = packet.encode_spandsp().unwrap();
            assert_eq!(
                encoded.len(),
                1,
                "spandsp format should be 1 byte for {:?}",
                ind
            );
            assert_eq!(encoded[0], val << 1, "mismatch for indicator {:?}", ind);
        }
    }

    #[test]
    fn test_encode_spandsp_rejects_multi_indicator() {
        let packet = IfpPacket::T30Indicator(vec![T30Indicator::Cng, T30Indicator::Ced]);
        assert!(packet.encode_spandsp().is_err());
    }

    #[test]
    fn test_encode_spandsp_rejects_data() {
        let packet = IfpPacket::T30Data(vec![DataField {
            field_type: DataFieldType::HdlcFcsOk,
            data: vec![],
        }]);
        assert!(packet.encode_spandsp().is_err());
    }

    #[test]
    fn test_ifp_encode_decode_multiple_fields() {
        let fields = (0..10)
            .map(|i| DataField {
                field_type: DataFieldType::from_u8(i % 7).unwrap(),
                data: vec![i as u8; ((i + 1) * 10) as usize],
            })
            .collect();
        let packet = IfpPacket::T30Data(fields);
        let encoded = packet.encode().unwrap();
        let decoded = IfpPacket::decode(&encoded).unwrap();
        assert_eq!(packet, decoded);
    }
}
