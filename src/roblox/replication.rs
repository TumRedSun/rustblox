//! Roblox replication stream parser (best-effort, version-tolerant).
//!
//! The live-server replication protocol is proprietary and version-gated.
//! This parser implements the framing layers that are stable knowledge
//! (packet header, zlib-compressed payloads) and walks the data stream
//! tolerantly: anything it cannot confidently decode is skipped and
//! hex-logged (RUSTBLOX_DEBUG=net) instead of desynchronizing the stream.
//!
//! The decoded events feed the DataModel. As live captures converge the
//! unknown structures, the decode rules here get upgraded — the design
//! goal is "never crash, always log".

use crate::netlog;
use crate::world::{DataModel, Prop, RefId};

/// A decoded replication event.
#[derive(Debug, Clone)]
pub enum RepEvent {
    InstanceCreated { r: RefId, class: String, name: String, parent: RefId },
    InstanceRemoved { r: RefId },
    PropertySet { r: RefId, name: String, value: Prop },
    /// Raw packet we could not decode (id + first bytes), kept for diagnostics.
    Unknown { packet_type: u32, len: usize },
}

pub struct ReplicationParser {
    /// Packets processed (any kind).
    pub packets: u64,
    /// Data packets fully decoded.
    pub decoded: u64,
    /// Chunks we had to skip.
    pub skipped: u64,
    /// How many hexdumps we already logged (limit the flood).
    pub hexdumps: u32,
}

impl ReplicationParser {
    pub fn new() -> ReplicationParser {
        ReplicationParser { packets: 0, decoded: 0, skipped: 0, hexdumps: 0 }
    }

    /// Feed one application payload (after RakNet deframing). Returns events.
    pub fn feed(&mut self, payload: &[u8], dm: &mut DataModel) -> Vec<RepEvent> {
        let mut events = Vec::new();
        self.feed_inner(payload, dm, &mut events, 0);
        events
    }

    fn feed_inner(&mut self, payload: &[u8], dm: &mut DataModel, events: &mut Vec<RepEvent>, depth: u32) {
        if payload.len() < 4 {
            return;
        }
        self.packets += 1;
        let header = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let ptype = header & 0x0f;
        let compressed = header & 0x8000_0000 != 0;
        let body = &payload[4..];

        let body = if compressed {
            match inflate(body) {
                Some(b) => b,
                None => {
                    self.skipped += 1;
                    self.log_hex("failed to inflate compressed packet", payload);
                    return;
                }
            }
        } else {
            body.to_vec()
        };

        match ptype {
            0 => self.parse_data(&body, dm, events, depth),
            4 | 5 => {
                // Ping/pong style service packets — informational only.
                netlog!(net, "service packet type {} ({} bytes)", ptype, body.len());
            }
            other => {
                events.push(RepEvent::Unknown { packet_type: other, len: body.len() });
                if self.hexdumps < 24 {
                    self.hexdumps += 1;
                    self.log_hex(&format!("packet type {}", other), &body);
                }
            }
        }
    }

    /// Walk a data packet: a sequence of clusters. Each cluster:
    ///   u32 length | u32 cluster kind | body
    /// Known cluster kinds (community knowledge, version-dependent):
    ///   1 = instance create, 2 = property set, 3 = instance delete,
    ///   5 = event, 7 = stats/heartbeat.
    fn parse_data(&mut self, body: &[u8], dm: &mut DataModel, events: &mut Vec<RepEvent>, depth: u32) {
        let mut pos = 0usize;
        let mut progress = true;
        while pos + 8 <= body.len() && progress {
            progress = false;
            let len = u32::from_le_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]]) as usize;
            if len == 0 || pos + 8 + len > body.len() {
                break;
            }
            let kind = u32::from_le_bytes([
                body[pos + 4],
                body[pos + 5],
                body[pos + 6],
                body[pos + 7],
            ]);
            let start = pos;
            pos += 8;
            let chunk = &body[pos..pos + len];
            match kind {
                1 => {
                    if let Some(ev) = self.parse_create(chunk, dm) {
                        events.push(ev);
                        self.decoded += 1;
                        progress = true;
                    } else {
                        self.skipped += 1;
                        self.log_hex("cluster create undecodable", &body[start..pos + len]);
                    }
                }
                2 => {
                    if let Some(ev) = self.parse_property(chunk, dm) {
                        events.push(ev);
                        self.decoded += 1;
                        progress = true;
                    } else {
                        self.skipped += 1;
                        self.log_hex("cluster property undecodable", &body[start..pos + len]);
                    }
                }
                3 => {
                    if chunk.len() >= 8 {
                        let r = i64::from_le_bytes(chunk[0..8].try_into().unwrap());
                        dm.remove(r);
                        events.push(RepEvent::InstanceRemoved { r });
                        self.decoded += 1;
                        progress = true;
                    }
                }
                _ => {
                    self.skipped += 1;
                    if self.hexdumps < 24 {
                        self.hexdumps += 1;
                        netlog!(net, "cluster kind {} ({} bytes) at {}", kind, len, pos);
                    }
                    progress = true; // keep walking; the framing looks sane
                }
            }
            pos += len;
        }
        if depth < 1 && body.len() >= 4 && self.skipped == 0 && self.decoded == 0 {
            self.log_hex("data packet produced nothing", body);
        }
    }

    /// Instance create cluster (tolerant):
    ///   i64 ref | u8 string-kind | u16 len | class | u16 len | name | i64 parent
    fn parse_create(&mut self, chunk: &[u8], dm: &mut DataModel) -> Option<RepEvent> {
        if chunk.len() < 8 + 1 + 2 {
            return None;
        }
        let mut pos = 0usize;
        let r = i64::from_le_bytes(match chunk[0..8].try_into() { Ok(v) => v, Err(_) => return None });
        pos += 8;
        // Try: u16 len + class + u16 len + name + i64 parent.
        let (class, pos2) = read_str16(chunk, pos)?;
        let (name, pos3) = read_str16(chunk, pos2)?;
        if pos3 + 8 > chunk.len() {
            return None;
        }
        let parent = i64::from_le_bytes(match chunk[pos3..pos3 + 8].try_into() { Ok(v) => v, Err(_) => return None });
        let _ = pos;
        dm.create(r, &class, &name, parent);
        Some(RepEvent::InstanceCreated { r, class, name, parent })
    }

    /// Property set cluster (tolerant):
    ///   i64 ref | u16 len name | u8 value kind | value bytes
    /// Value kinds: 1 string(u32), 2 bool, 3 f32, 4 f64, 5 vec3, 6 cframe,
    /// 7 color3, 8 i32, 9 ref, 10 color3u8, 11 token(u32), 12 vec2.
    fn parse_property(&mut self, chunk: &[u8], dm: &mut DataModel) -> Option<RepEvent> {
        if chunk.len() < 8 + 2 {
            return None;
        }
        let r = i64::from_le_bytes(match chunk[0..8].try_into() { Ok(v) => v, Err(_) => return None });
        let (name, mut pos) = read_str16(chunk, 8)?;
        if pos >= chunk.len() {
            return None;
        }
        let kind = chunk[pos];
        pos += 1;
        let rest = &chunk[pos..];
        let value = match kind {
            1 => {
                if rest.len() < 4 {
                    return None;
                }
                let n = u32::from_le_bytes(match rest[0..4].try_into() { Ok(v) => v, Err(_) => return None }) as usize;
                if rest.len() < 4 + n {
                    return None;
                }
                Prop::Str(String::from_utf8_lossy(&rest[4..4 + n]).to_string())
            }
            2 => Prop::Bool(rest.first()? != &0),
            3 => Prop::F32(f32::from_le_bytes(match rest.get(0..4)?.try_into() { Ok(v) => v, Err(_) => return None })),
            4 => Prop::F64(f64::from_le_bytes(match rest.get(0..8)?.try_into() { Ok(v) => v, Err(_) => return None })),
            5 => Prop::Vec3([
                f32::from_le_bytes(match rest.get(0..4)?.try_into() { Ok(v) => v, Err(_) => return None }),
                f32::from_le_bytes(match rest.get(4..8)?.try_into() { Ok(v) => v, Err(_) => return None }),
                f32::from_le_bytes(match rest.get(8..12)?.try_into() { Ok(v) => v, Err(_) => return None }),
            ]),
            6 => {
                if rest.len() < 48 {
                    return None;
                }
                let mut f = [0f32; 12];
                for (i, v) in f.iter_mut().enumerate() {
                    *v = f32::from_le_bytes(match rest[i * 4..i * 4 + 4].try_into() { Ok(v) => v, Err(_) => return None });
                }
                // Layout guess: pos (3) + rotation rows (9).
                let pos = [f[0], f[1], f[2]];
                let rot = [f[3], f[4], f[5], f[6], f[7], f[8], f[9], f[10], f[11]];
                Prop::CFrame { pos, rot }
            }
            7 => Prop::Color3([
                f32::from_le_bytes(match rest.get(0..4)?.try_into() { Ok(v) => v, Err(_) => return None }),
                f32::from_le_bytes(match rest.get(4..8)?.try_into() { Ok(v) => v, Err(_) => return None }),
                f32::from_le_bytes(match rest.get(8..12)?.try_into() { Ok(v) => v, Err(_) => return None }),
            ]),
            8 => Prop::I32(i32::from_le_bytes(match rest.get(0..4)?.try_into() { Ok(v) => v, Err(_) => return None })),
            9 => Prop::Ref(i64::from_le_bytes(match rest.get(0..8)?.try_into() { Ok(v) => v, Err(_) => return None })),
            10 => Prop::Color3u8([*rest.first()?, *rest.get(1)?, *rest.get(2)?]),
            11 => Prop::Token(u32::from_le_bytes(match rest.get(0..4)?.try_into() { Ok(v) => v, Err(_) => return None })),
            12 => Prop::Vec2([
                f32::from_le_bytes(match rest.get(0..4)?.try_into() { Ok(v) => v, Err(_) => return None }),
                f32::from_le_bytes(match rest.get(4..8)?.try_into() { Ok(v) => v, Err(_) => return None }),
            ]),
            _ => Prop::Raw(rest.to_vec()),
        };
        dm.set_prop(r, &name, value.clone());
        Some(RepEvent::PropertySet { r, name, value })
    }

    fn log_hex(&mut self, label: &str, data: &[u8]) {
        if self.hexdumps >= 24 {
            return;
        }
        self.hexdumps += 1;
        let take = data.len().min(64);
        let hex: String = data[..take].iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ");
        netlog!(net, "HEX {} ({} bytes): {}", label, data.len(), hex);
    }
}

fn read_str16(buf: &[u8], pos: usize) -> Option<(String, usize)> {
    if pos + 2 > buf.len() {
        return None;
    }
    let n = u16::from_le_bytes([buf[pos], buf[pos + 1]]) as usize;
    if pos + 2 + n > buf.len() {
        return None;
    }
    Some((String::from_utf8_lossy(&buf[pos + 2..pos + 2 + n]).to_string(), pos + 2 + n))
}

fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    match flate2::read::ZlibDecoder::new(data).take(64 * 1024 * 1024).read_to_end(&mut out) {
        Ok(_) => Some(out),
        Err(_) => {
            // Try raw deflate as a fallback.
            let mut out2 = Vec::new();
            match flate2::read::DeflateDecoder::new(data).take(64 * 1024 * 1024).read_to_end(&mut out2) {
                Ok(_) => Some(out2),
                Err(_) => None,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn u32le(v: u32) -> [u8; 4] {
        v.to_le_bytes()
    }

    #[test]
    fn create_and_property_events() {
        let mut p = ReplicationParser::new();
        let mut dm = DataModel::new();

        // cluster 1: create Part "Brick" ref 42 parent 41
        let mut create_body = Vec::new();
        create_body.extend_from_slice(&42i64.to_le_bytes());
        create_body.extend_from_slice(&4u16.to_le_bytes());
        create_body.extend_from_slice(b"Part");
        create_body.extend_from_slice(&5u16.to_le_bytes());
        create_body.extend_from_slice(b"Brick");
        create_body.extend_from_slice(&41i64.to_le_bytes());
        let mut cluster = Vec::new();
        cluster.extend_from_slice(&u32le(create_body.len() as u32));
        cluster.extend_from_slice(&u32le(1));
        cluster.extend_from_slice(&create_body);

        // cluster 2: property size vec3 on ref 42
        let mut prop_body = Vec::new();
        prop_body.extend_from_slice(&42i64.to_le_bytes());
        prop_body.extend_from_slice(&4u16.to_le_bytes());
        prop_body.extend_from_slice(b"Size");
        prop_body.push(5);
        prop_body.extend_from_slice(&[4.0f32, 1.2, 2.0].iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>());
        cluster.extend_from_slice(&u32le(prop_body.len() as u32));
        cluster.extend_from_slice(&u32le(2));
        cluster.extend_from_slice(&prop_body);

        let mut packet = u32le(0).to_vec();
        packet.extend_from_slice(&cluster);
        let events = p.feed(&packet, &mut dm);
        assert_eq!(events.len(), 2);
        let brick = dm.get_by_ref(42).unwrap();
        assert_eq!(brick.class, "Part");
        assert_eq!(brick.name, "Brick");
        assert_eq!(brick.prop("Size"), Some(&Prop::Vec3([4.0, 1.2, 2.0])));
        assert_eq!(p.decoded, 2);
    }

    #[test]
    fn compressed_packet_inflates() {
        use flate2::write::DeflateEncoder;
        use flate2::Compression;
        use std::io::Write;
        let mut p = ReplicationParser::new();
        let mut dm = DataModel::new();

        let mut create_body = Vec::new();
        create_body.extend_from_slice(&7i64.to_le_bytes());
        create_body.extend_from_slice(&4u16.to_le_bytes());
        create_body.extend_from_slice(b"Part");
        create_body.extend_from_slice(&1u16.to_le_bytes());
        create_body.extend_from_slice(b"P");
        create_body.extend_from_slice(&(-1i64).to_le_bytes());
        let mut data_packet = Vec::new();
        data_packet.extend_from_slice(&u32le(create_body.len() as u32));
        data_packet.extend_from_slice(&u32le(1));
        data_packet.extend_from_slice(&create_body);

        let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&data_packet).unwrap();
        let compressed = enc.finish().unwrap();

        let mut packet = u32le(0x8000_0000).to_vec(); // type 0 + compressed bit
        packet.extend_from_slice(&compressed);
        let events = p.feed(&packet, &mut dm);
        assert_eq!(events.len(), 1);
        assert!(dm.get_by_ref(7).is_some());
    }

    #[test]
    fn unknown_types_are_tolerated() {
        let mut p = ReplicationParser::new();
        let mut dm = DataModel::new();
        let packet = u32le(9).to_vec();
        let events = p.feed(&packet, &mut dm);
        assert!(matches!(events[0], RepEvent::Unknown { packet_type: 9, .. }));
        // Stream must stay alive for more packets.
        let events2 = p.feed(&u32le(9).to_vec(), &mut dm);
        assert_eq!(events2.len(), 1);
    }

    #[test]
    fn delete_event() {
        let mut p = ReplicationParser::new();
        let mut dm = DataModel::new();
        dm.create(55, "Part", "x", -1);
        let mut cluster = Vec::new();
        let body = 55i64.to_le_bytes();
        cluster.extend_from_slice(&u32le(8));
        cluster.extend_from_slice(&u32le(3));
        cluster.extend_from_slice(&body);
        let mut packet = u32le(0).to_vec();
        packet.extend_from_slice(&cluster);
        let events = p.feed(&packet, &mut dm);
        assert!(matches!(events[0], RepEvent::InstanceRemoved { r: 55 }));
        assert!(dm.get_by_ref(55).is_none());
    }
}
