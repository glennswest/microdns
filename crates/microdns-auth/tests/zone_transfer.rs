//! End-to-end cover for the primary/secondary pair: a NOTIFY arrives, the
//! secondary checks the primary's serial, and the zone lands in its database.
//!
//! These run two real servers over loopback rather than mocking the wire,
//! because every interesting failure in this feature lives in the wire format
//! or the ACL, and a mock would agree with whatever the code does.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use chrono::Utc;
use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::{Name, RecordType as ProtoType};
use hickory_proto::serialize::binary::{BinDecodable, BinEncodable};
use microdns_auth::runtime::TransferState;
use microdns_auth::secondary::SecondaryAgent;
use microdns_auth::server::AuthServer;
use microdns_auth::transfer::ZoneTransfer;
use microdns_core::db::Db;
use microdns_core::types::{Record, RecordData, RecordSource, SoaData, Zone};
use std::str::FromStr;
use tokio::sync::watch;
use uuid::Uuid;

/// Grab a port the OS says is free. A fixed port would collide with whatever
/// else is running on a developer's machine.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.local_addr().unwrap().port()
}

fn test_db() -> (Db, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("test.redb")).unwrap();
    (db, dir)
}

/// A primary holding `gw.lo` with two hosts in it.
fn seed_primary(db: &Db, serial: u32) -> Zone {
    let zone = Zone {
        id: Uuid::new_v4(),
        name: "gw.lo".to_string(),
        soa: SoaData {
            mname: "ns1.gw.lo".into(),
            rname: "admin.gw.lo".into(),
            serial,
            refresh: 3600,
            retry: 900,
            expire: 604800,
            minimum: 300,
        },
        default_ttl: 300,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    db.create_zone("gw.lo", &zone).unwrap();

    for (name, ip) in [("boot", "192.168.1.5"), ("registry", "192.168.1.80")] {
        db.create_record(&Record {
            id: Uuid::new_v4(),
            zone_id: zone.id,
            name: name.into(),
            ttl: 300,
            data: RecordData::A(ip.parse().unwrap()),
            enabled: true,
            health_check: None,
            source: RecordSource::Manual,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();
    }
    zone
}

/// Start an auth server on loopback and wait until it answers.
async fn start_primary(db: Db, allow_transfer: &[String]) -> (SocketAddr, watch::Sender<bool>) {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, free_port()));
    let (tx, rx) = watch::channel(false);
    let server = AuthServer::new(addr, db).with_allow_transfer(allow_transfer);
    tokio::spawn(async move {
        let _ = server.run(rx).await;
    });

    // Poll rather than sleep a fixed amount: the bind is fast but not instant.
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    (addr, tx)
}

#[tokio::test]
async fn a_secondary_pulls_the_zone_over_axfr() {
    let (primary_db, _p) = test_db();
    seed_primary(&primary_db, 2026081801);
    let (addr, _stop) = start_primary(primary_db, &["127.0.0.0/8".to_string()]).await;

    let (secondary_db, _s) = test_db();
    let result = ZoneTransfer::new(secondary_db.clone())
        .axfr_pull("gw.lo", addr)
        .await
        .expect("transfer should succeed");

    assert_eq!(result.records_imported, 2);
    assert_eq!(result.serial, 2026081801);

    let zone = secondary_db.get_zone_by_name("gw.lo").unwrap().unwrap();
    assert_eq!(zone.soa.serial, 2026081801);
    let records = secondary_db
        .query_fqdn("boot.gw.lo", microdns_core::types::RecordType::A)
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].data, RecordData::A("192.168.1.5".parse().unwrap()));
}

#[tokio::test]
async fn a_transfer_from_a_denied_address_fails() {
    let (primary_db, _p) = test_db();
    seed_primary(&primary_db, 1);
    // Loopback is deliberately not in the allow-list.
    let (addr, _stop) = start_primary(primary_db, &["10.0.0.0/8".to_string()]).await;

    let (secondary_db, _s) = test_db();
    let result = ZoneTransfer::new(secondary_db.clone())
        .axfr_pull("gw.lo", addr)
        .await;

    assert!(result.is_err(), "AXFR must be refused for a denied peer");
    assert!(
        secondary_db.get_zone_by_name("gw.lo").unwrap().is_none(),
        "a refused transfer must not leave a half-built zone behind"
    );
}

#[tokio::test]
async fn a_second_transfer_replaces_the_previous_copy() {
    let (primary_db, _p) = test_db();
    let zone = seed_primary(&primary_db, 10);
    let (addr, _stop) = start_primary(primary_db.clone(), &["127.0.0.0/8".to_string()]).await;

    let (secondary_db, _s) = test_db();
    let transfer = ZoneTransfer::new(secondary_db.clone());
    transfer.axfr_pull("gw.lo", addr).await.unwrap();
    let first_id = secondary_db.get_zone_by_name("gw.lo").unwrap().unwrap().id;

    // The primary drops a record and bumps its serial.
    let boot = primary_db
        .query_fqdn("boot.gw.lo", microdns_core::types::RecordType::A)
        .unwrap()[0]
        .clone();
    primary_db.delete_record(&boot.id).unwrap();
    primary_db.increment_soa_serial(&zone.id).unwrap();

    let result = transfer.axfr_pull("gw.lo", addr).await.unwrap();
    assert_eq!(result.records_imported, 1);

    let records = secondary_db.list_records(&first_id).unwrap();
    assert_eq!(records.len(), 1, "the withdrawn record must be gone");
    assert_eq!(
        secondary_db.get_zone_by_name("gw.lo").unwrap().unwrap().id,
        first_id,
        "the zone keeps its identity across transfers"
    );
}

/// Send a NOTIFY the way a primary would and return the response.
async fn send_notify(target: SocketAddr, zone: &str) -> Message {
    let mut query = Query::new();
    query.set_name(Name::from_str(&format!("{zone}.")).unwrap());
    query.set_query_type(ProtoType::SOA);

    let mut message = Message::new();
    message.set_id(4242);
    message.set_message_type(MessageType::Query);
    message.set_op_code(OpCode::Notify);
    message.set_authoritative(true);
    message.add_query(query);

    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.send_to(&message.to_bytes().unwrap(), target).await.unwrap();

    let mut buf = vec![0u8; 4096];
    let len = tokio::time::timeout(Duration::from_secs(3), socket.recv(&mut buf))
        .await
        .expect("NOTIFY should be answered")
        .unwrap();
    Message::from_bytes(&buf[..len]).unwrap()
}

#[tokio::test]
async fn a_notify_from_the_primary_makes_the_secondary_transfer() {
    // The primary: holds gw.lo, permits transfers from loopback.
    let (primary_db, _p) = test_db();
    seed_primary(&primary_db, 2026081801);
    let (primary_addr, _stop_primary) =
        start_primary(primary_db, &["127.0.0.0/8".to_string()]).await;

    // The secondary: mirrors gw.lo from that primary, and runs its own DNS
    // listener so a NOTIFY has somewhere to land.
    let (secondary_db, _s) = test_db();
    let state = TransferState::new(&microdns_core::config::ZoneTransferConfig {
        allow_transfer: vec![],
        notify: vec![],
        secondary: vec![microdns_core::config::SecondaryZoneConfig {
            zone: "gw.lo".into(),
            primary: primary_addr.to_string(),
            refresh_secs: 3600,
        }],
    });
    let (agent, acceptor) = SecondaryAgent::new(secondary_db.clone(), state);

    let secondary_addr = SocketAddr::from((Ipv4Addr::LOCALHOST, free_port()));
    let (_stop_secondary, server_rx) = watch::channel(false);
    let server = AuthServer::new(secondary_addr, secondary_db.clone())
        .with_notify_acceptor(acceptor);
    tokio::spawn(async move {
        let _ = server.run(server_rx).await;
    });
    let (_stop_agent, agent_rx) = watch::channel(false);
    tokio::spawn(async move {
        let _ = agent.run(agent_rx).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let response = send_notify(secondary_addr, "gw.lo").await;
    assert_eq!(response.op_code(), OpCode::Notify);
    assert_eq!(response.response_code(), ResponseCode::NoError);
    assert_eq!(response.id(), 4242, "the reply must echo the request id");

    // The agent settles briefly before transferring, so give it room.
    let mut transferred = false;
    for _ in 0..40 {
        if secondary_db.get_zone_by_name("gw.lo").unwrap().is_some() {
            transferred = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(transferred, "the NOTIFY should have driven a transfer");

    let records = secondary_db
        .query_fqdn("registry.gw.lo", microdns_core::types::RecordType::A)
        .unwrap();
    assert_eq!(records.len(), 1);
}

#[tokio::test]
async fn a_notify_from_anyone_else_is_refused() {
    let (secondary_db, _s) = test_db();
    // The configured primary is an address this test cannot send from.
    let state = TransferState::new(&microdns_core::config::ZoneTransferConfig {
        allow_transfer: vec![],
        notify: vec![],
        secondary: vec![microdns_core::config::SecondaryZoneConfig {
            zone: "gw.lo".into(),
            primary: "192.0.2.1".into(),
            refresh_secs: 3600,
        }],
    });
    let (_agent, acceptor) = SecondaryAgent::new(secondary_db.clone(), state);

    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, free_port()));
    let (_stop, rx) = watch::channel(false);
    let server = AuthServer::new(addr, secondary_db.clone()).with_notify_acceptor(acceptor);
    tokio::spawn(async move {
        let _ = server.run(rx).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let response = send_notify(addr, "gw.lo").await;
    assert_eq!(response.response_code(), ResponseCode::Refused);

    // And a zone we do not mirror at all is refused too.
    let response = send_notify(addr, "example.com").await;
    assert_eq!(response.response_code(), ResponseCode::Refused);
}
