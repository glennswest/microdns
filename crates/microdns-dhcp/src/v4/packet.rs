use std::net::Ipv4Addr;

/// DHCP message types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DhcpMessageType {
    Discover = 1,
    Offer = 2,
    Request = 3,
    Decline = 4,
    Ack = 5,
    Nak = 6,
    Release = 7,
    Inform = 8,
}

impl DhcpMessageType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Discover),
            2 => Some(Self::Offer),
            3 => Some(Self::Request),
            4 => Some(Self::Decline),
            5 => Some(Self::Ack),
            6 => Some(Self::Nak),
            7 => Some(Self::Release),
            8 => Some(Self::Inform),
            _ => None,
        }
    }
}

/// DHCP option codes
pub const OPT_SUBNET_MASK: u8 = 1;
pub const OPT_ROUTER: u8 = 3;
pub const OPT_DNS_SERVER: u8 = 6;
pub const OPT_HOSTNAME: u8 = 12;
pub const OPT_DOMAIN_NAME: u8 = 15;
pub const OPT_REQUESTED_IP: u8 = 50;
pub const OPT_LEASE_TIME: u8 = 51;
pub const OPT_MESSAGE_TYPE: u8 = 53;
pub const OPT_SERVER_ID: u8 = 54;
pub const OPT_PARAMETER_LIST: u8 = 55;
pub const OPT_TFTP_SERVER: u8 = 66;
pub const OPT_BOOTFILE: u8 = 67;
pub const OPT_END: u8 = 255;

/// Magic cookie for DHCP options
const MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];

/// Parsed DHCP packet
#[derive(Debug, Clone)]
pub struct DhcpPacket {
    pub op: u8,         // 1=BOOTREQUEST, 2=BOOTREPLY
    pub htype: u8,      // Hardware type (1=Ethernet)
    pub hlen: u8,       // Hardware address length (6 for MAC)
    pub hops: u8,
    pub xid: u32,       // Transaction ID
    pub secs: u16,
    pub flags: u16,
    pub ciaddr: Ipv4Addr, // Client IP
    pub yiaddr: Ipv4Addr, // 'Your' IP (offered/assigned)
    pub siaddr: Ipv4Addr, // Server IP
    pub giaddr: Ipv4Addr, // Gateway/relay IP
    pub chaddr: [u8; 16], // Client hardware address
    pub sname: [u8; 64],  // Server host name (PXE boot)
    pub file: [u8; 128],  // Boot file name (PXE boot)
    pub options: Vec<DhcpOption>,
}

#[derive(Debug, Clone)]
pub struct DhcpOption {
    pub code: u8,
    pub data: Vec<u8>,
}

impl DhcpPacket {
    /// Parse a DHCP packet from raw bytes.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 240 {
            return None;
        }

        let op = data[0];
        let htype = data[1];
        let hlen = data[2];
        let hops = data[3];
        let xid = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let secs = u16::from_be_bytes([data[8], data[9]]);
        let flags = u16::from_be_bytes([data[10], data[11]]);
        let ciaddr = Ipv4Addr::new(data[12], data[13], data[14], data[15]);
        let yiaddr = Ipv4Addr::new(data[16], data[17], data[18], data[19]);
        let siaddr = Ipv4Addr::new(data[20], data[21], data[22], data[23]);
        let giaddr = Ipv4Addr::new(data[24], data[25], data[26], data[27]);

        let mut chaddr = [0u8; 16];
        chaddr.copy_from_slice(&data[28..44]);

        let mut sname = [0u8; 64];
        sname.copy_from_slice(&data[44..108]);

        let mut file = [0u8; 128];
        file.copy_from_slice(&data[108..236]);

        // Options start at offset 236, after magic cookie at 236..240
        if data[236..240] != MAGIC_COOKIE {
            return None;
        }

        let options = parse_options(&data[240..])?;

        Some(DhcpPacket {
            op,
            htype,
            hlen,
            hops,
            xid,
            secs,
            flags,
            ciaddr,
            yiaddr,
            siaddr,
            giaddr,
            chaddr,
            sname,
            file,
            options,
        })
    }

    /// Serialize to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = vec![0u8; 240];

        buf[0] = self.op;
        buf[1] = self.htype;
        buf[2] = self.hlen;
        buf[3] = self.hops;
        buf[4..8].copy_from_slice(&self.xid.to_be_bytes());
        buf[8..10].copy_from_slice(&self.secs.to_be_bytes());
        buf[10..12].copy_from_slice(&self.flags.to_be_bytes());
        buf[12..16].copy_from_slice(&self.ciaddr.octets());
        buf[16..20].copy_from_slice(&self.yiaddr.octets());
        buf[20..24].copy_from_slice(&self.siaddr.octets());
        buf[24..28].copy_from_slice(&self.giaddr.octets());
        buf[28..44].copy_from_slice(&self.chaddr);
        buf[44..108].copy_from_slice(&self.sname);
        buf[108..236].copy_from_slice(&self.file);

        // Magic cookie
        buf[236..240].copy_from_slice(&MAGIC_COOKIE);

        // Options
        for opt in &self.options {
            buf.push(opt.code);
            if opt.code != OPT_END {
                buf.push(opt.data.len() as u8);
                buf.extend_from_slice(&opt.data);
            }
        }

        // End option
        if self.options.last().map(|o| o.code) != Some(OPT_END) {
            buf.push(OPT_END);
        }

        // Pad to minimum 300 bytes
        while buf.len() < 300 {
            buf.push(0);
        }

        buf
    }

    /// Get the DHCP message type from options.
    pub fn message_type(&self) -> Option<DhcpMessageType> {
        self.get_option(OPT_MESSAGE_TYPE)
            .and_then(|data| data.first().copied())
            .and_then(DhcpMessageType::from_u8)
    }

    /// Get requested IP address from options.
    pub fn requested_ip(&self) -> Option<Ipv4Addr> {
        self.get_option(OPT_REQUESTED_IP).and_then(|data| {
            if data.len() == 4 {
                Some(Ipv4Addr::new(data[0], data[1], data[2], data[3]))
            } else {
                None
            }
        })
    }

    /// Get hostname from options.
    pub fn hostname(&self) -> Option<String> {
        self.get_option(OPT_HOSTNAME)
            .and_then(|data| String::from_utf8(data.to_vec()).ok())
    }

    /// Get a specific option's data.
    pub fn get_option(&self, code: u8) -> Option<&[u8]> {
        self.options
            .iter()
            .find(|o| o.code == code)
            .map(|o| o.data.as_slice())
    }

    /// Get the MAC address as a string.
    pub fn mac_address(&self) -> String {
        let len = self.hlen as usize;
        let mac = &self.chaddr[..len.min(6)];
        mac.iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":")
    }
}

fn parse_options(data: &[u8]) -> Option<Vec<DhcpOption>> {
    let mut options = Vec::new();
    let mut i = 0;

    while i < data.len() {
        let code = data[i];
        i += 1;

        if code == OPT_END {
            options.push(DhcpOption {
                code: OPT_END,
                data: Vec::new(),
            });
            break;
        }

        if code == 0 {
            // Pad option
            continue;
        }

        if i >= data.len() {
            break;
        }

        let len = data[i] as usize;
        i += 1;

        if i + len > data.len() {
            break;
        }

        options.push(DhcpOption {
            code,
            data: data[i..i + len].to_vec(),
        });
        i += len;
    }

    Some(options)
}

/// Build a DHCP option with an IPv4 address value.
pub fn ip_option(code: u8, addr: Ipv4Addr) -> DhcpOption {
    DhcpOption {
        code,
        data: addr.octets().to_vec(),
    }
}

/// Build a DHCP option with a u32 value (e.g., lease time).
pub fn u32_option(code: u8, val: u32) -> DhcpOption {
    DhcpOption {
        code,
        data: val.to_be_bytes().to_vec(),
    }
}

/// Build a DHCP option with a list of IPv4 addresses.
pub fn ip_list_option(code: u8, addrs: &[Ipv4Addr]) -> DhcpOption {
    let mut data = Vec::new();
    for addr in addrs {
        data.extend_from_slice(&addr.octets());
    }
    DhcpOption { code, data }
}

/// Build a DHCP option with a string value.
pub fn string_option(code: u8, s: &str) -> DhcpOption {
    DhcpOption {
        code,
        data: s.as_bytes().to_vec(),
    }
}

/// Build a message type option.
pub fn message_type_option(msg_type: DhcpMessageType) -> DhcpOption {
    DhcpOption {
        code: OPT_MESSAGE_TYPE,
        data: vec![msg_type as u8],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        let packet = DhcpPacket {
            op: 1,
            htype: 1,
            hlen: 6,
            hops: 0,
            xid: 0x12345678,
            secs: 0,
            flags: 0x8000,
            ciaddr: Ipv4Addr::UNSPECIFIED,
            yiaddr: Ipv4Addr::UNSPECIFIED,
            siaddr: Ipv4Addr::UNSPECIFIED,
            giaddr: Ipv4Addr::UNSPECIFIED,
            chaddr: {
                let mut c = [0u8; 16];
                c[0..6].copy_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
                c
            },
            sname: [0u8; 64],
            file: [0u8; 128],
            options: vec![
                message_type_option(DhcpMessageType::Discover),
                DhcpOption {
                    code: OPT_END,
                    data: Vec::new(),
                },
            ],
        };

        let bytes = packet.to_bytes();
        let parsed = DhcpPacket::parse(&bytes).unwrap();

        assert_eq!(parsed.op, 1);
        assert_eq!(parsed.xid, 0x12345678);
        assert_eq!(parsed.flags, 0x8000);
        assert_eq!(parsed.message_type(), Some(DhcpMessageType::Discover));
        assert_eq!(parsed.mac_address(), "aa:bb:cc:dd:ee:ff");
    }

    #[test]
    fn test_pxe_fields_roundtrip() {
        let mut sname = [0u8; 64];
        let sname_str = b"pxeserver";
        sname[..sname_str.len()].copy_from_slice(sname_str);

        let mut file = [0u8; 128];
        let file_str = b"pxelinux.0";
        file[..file_str.len()].copy_from_slice(file_str);

        let packet = DhcpPacket {
            op: 2,
            htype: 1,
            hlen: 6,
            hops: 0,
            xid: 0xDEADBEEF,
            secs: 0,
            flags: 0,
            ciaddr: Ipv4Addr::UNSPECIFIED,
            yiaddr: "10.0.10.100".parse().unwrap(),
            siaddr: "10.0.10.5".parse().unwrap(),
            giaddr: Ipv4Addr::UNSPECIFIED,
            chaddr: [0u8; 16],
            sname,
            file,
            options: vec![
                message_type_option(DhcpMessageType::Offer),
                string_option(OPT_TFTP_SERVER, "10.0.10.5"),
                string_option(OPT_BOOTFILE, "pxelinux.0"),
                DhcpOption { code: OPT_END, data: Vec::new() },
            ],
        };

        let bytes = packet.to_bytes();
        let parsed = DhcpPacket::parse(&bytes).unwrap();

        assert_eq!(parsed.siaddr, "10.0.10.5".parse::<Ipv4Addr>().unwrap());
        assert_eq!(&parsed.sname[..sname_str.len()], sname_str);
        assert_eq!(&parsed.file[..file_str.len()], file_str);
        assert_eq!(
            parsed.get_option(OPT_TFTP_SERVER).map(|d| std::str::from_utf8(d).unwrap()),
            Some("10.0.10.5")
        );
        assert_eq!(
            parsed.get_option(OPT_BOOTFILE).map(|d| std::str::from_utf8(d).unwrap()),
            Some("pxelinux.0")
        );
    }
}
