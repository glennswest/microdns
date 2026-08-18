//! Multicast sockets and outbound queries.
//!
//! The listener binds the mDNS port with `SO_REUSEADDR`/`SO_REUSEPORT` and
//! joins the group, which is what lets it coexist with any other mDNS stack on
//! the host — the protocol assumes several listeners share the port.
//!
//! Queries go out from the same socket, so the source port is 5353 and
//! responders answer with a normal multicast response (RFC 6762 §6.7 treats a
//! query from any other port as a legacy one and answers it with a 10-second
//! TTL, which is not what we want to be publishing).

use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use hickory_proto::op::{Message, MessageType, OpCode, Query};
use hickory_proto::rr::{DNSClass, Name, RecordType as ProtoType};
use hickory_proto::serialize::binary::BinEncodable;
use microdns_core::types::RecordType;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tracing::{debug, warn};

use crate::config::{MdnsConfig, MDNS_GROUP_V4, MDNS_GROUP_V6};

/// The largest mDNS packet worth reading. RFC 6762 §17 allows responses up to
/// the interface MTU, and jumbo-framed segments exist.
pub const MAX_PACKET: usize = 9000;

/// Bind the IPv4 listener and join `224.0.0.251` on the configured interfaces.
pub fn bind_v4(config: &MdnsConfig) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddrV4::new(config.bind, config.port).into())?;

    // An empty interface list means "let the kernel choose", which is what a
    // container with one interface wants; naming interfaces matters only on a
    // multi-homed host where the wrong choice would listen to the wrong LAN.
    let interfaces: Vec<Ipv4Addr> = if config.interfaces.is_empty() {
        vec![Ipv4Addr::UNSPECIFIED]
    } else {
        config.interfaces.clone()
    };
    let mut joined = 0;
    for iface in &interfaces {
        match socket.join_multicast_v4(&MDNS_GROUP_V4, iface) {
            Ok(()) => joined += 1,
            Err(e) => warn!("mdns: joining {MDNS_GROUP_V4} on {iface} failed: {e}"),
        }
    }
    if joined == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("could not join {MDNS_GROUP_V4} on any interface"),
        ));
    }
    if let Some(iface) = config.interfaces.first() {
        socket.set_multicast_if_v4(iface)?;
    }
    // RFC 6762 §11: mDNS packets carry TTL 255 so a receiver can tell they were
    // not forwarded by a router.
    socket.set_multicast_ttl_v4(255)?;

    UdpSocket::from_std(socket.into())
}

/// Bind the IPv6 listener and join `ff02::fb`.
pub fn bind_v6(config: &MdnsConfig) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_only_v6(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddrV6::new(std::net::Ipv6Addr::UNSPECIFIED, config.port, 0, 0).into())?;
    socket.join_multicast_v6(&MDNS_GROUP_V6, 0)?;
    socket.set_multicast_hops_v6(255)?;

    UdpSocket::from_std(socket.into())
}

/// The group address queries are sent to.
pub fn group_v4(port: u16) -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(MDNS_GROUP_V4, port))
}

/// The IPv6 group address queries are sent to.
pub fn group_v6(port: u16) -> SocketAddr {
    SocketAddr::V6(SocketAddrV6::new(MDNS_GROUP_V6, port, 0, 0))
}

/// Build a multicast query for a batch of `(name, type)` pairs.
///
/// mDNS queries carry ID 0 and no recursion — responders match on the question
/// alone (RFC 6762 §18.1).
pub fn query(questions: &[(String, RecordType)]) -> Option<Vec<u8>> {
    let mut message = Message::new();
    message.set_id(0);
    message.set_message_type(MessageType::Query);
    message.set_op_code(OpCode::Query);
    message.set_recursion_desired(false);

    let mut added = 0;
    for (name, rtype) in questions {
        let Ok(parsed) = Name::from_utf8(ensure_root(name)) else {
            debug!("mdns: skipping unqueryable name '{name}'");
            continue;
        };
        let mut q = Query::new();
        q.set_name(parsed);
        q.set_query_type(proto_type(*rtype));
        q.set_query_class(DNSClass::IN);
        message.add_query(q);
        added += 1;
    }
    if added == 0 {
        return None;
    }

    match message.to_bytes() {
        Ok(wire) => Some(wire),
        Err(e) => {
            warn!("mdns: could not encode query: {e}");
            None
        }
    }
}

fn ensure_root(name: &str) -> String {
    if name.ends_with('.') {
        name.to_string()
    } else {
        format!("{name}.")
    }
}

fn proto_type(rtype: RecordType) -> ProtoType {
    match rtype {
        RecordType::A => ProtoType::A,
        RecordType::AAAA => ProtoType::AAAA,
        RecordType::CNAME => ProtoType::CNAME,
        RecordType::MX => ProtoType::MX,
        RecordType::NS => ProtoType::NS,
        RecordType::PTR => ProtoType::PTR,
        RecordType::SOA => ProtoType::SOA,
        RecordType::SRV => ProtoType::SRV,
        RecordType::TXT => ProtoType::TXT,
        RecordType::CAA => ProtoType::CAA,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::serialize::binary::BinDecodable;

    #[test]
    fn a_query_carries_id_zero_and_every_question() {
        let wire = query(&[
            ("_services._dns-sd._udp.local".into(), RecordType::PTR),
            ("tracker.local".into(), RecordType::A),
        ])
        .unwrap();

        let parsed = Message::from_bytes(&wire).unwrap();
        assert_eq!(parsed.id(), 0);
        assert_eq!(parsed.message_type(), MessageType::Query);
        assert_eq!(parsed.queries().len(), 2);
        assert_eq!(
            parsed.queries()[0].name().to_string(),
            "_services._dns-sd._udp.local."
        );
        assert_eq!(parsed.queries()[1].query_type(), ProtoType::A);
    }

    #[test]
    fn a_query_with_nothing_to_ask_is_not_sent() {
        assert!(query(&[]).is_none());
    }

    #[tokio::test]
    async fn binding_the_listener_works_on_an_ephemeral_port() {
        // Port 0 keeps the test off 5353 so it cannot collide with a real
        // responder on the build host.
        let config = MdnsConfig {
            port: 0,
            ..Default::default()
        };
        let socket = bind_v4(&config).expect("bind mDNS listener");
        assert!(socket.local_addr().unwrap().port() > 0);
    }
}
