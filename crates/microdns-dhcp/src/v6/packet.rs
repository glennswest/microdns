use std::net::Ipv6Addr;

/// DHCPv6 message types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Dhcpv6MessageType {
    Solicit = 1,
    Advertise = 2,
    Request = 3,
    Confirm = 4,
    Renew = 5,
    Rebind = 6,
    Reply = 7,
    Release = 8,
    Decline = 9,
    InformationRequest = 11,
}

impl Dhcpv6MessageType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Solicit),
            2 => Some(Self::Advertise),
            3 => Some(Self::Request),
            4 => Some(Self::Confirm),
            5 => Some(Self::Renew),
            6 => Some(Self::Rebind),
            7 => Some(Self::Reply),
            8 => Some(Self::Release),
            9 => Some(Self::Decline),
            11 => Some(Self::InformationRequest),
            _ => None,
        }
    }
}

/// DHCPv6 option codes
pub const OPT_CLIENTID: u16 = 1;
pub const OPT_SERVERID: u16 = 2;
pub const OPT_IA_NA: u16 = 3;
pub const OPT_IAADDR: u16 = 5;
pub const OPT_ORO: u16 = 6;
pub const OPT_DNS_SERVERS: u16 = 23;
pub const OPT_DOMAIN_LIST: u16 = 24;

/// Parsed DHCPv6 message
#[derive(Debug, Clone)]
pub struct Dhcpv6Packet {
    pub msg_type: u8,
    pub transaction_id: [u8; 3],
    pub options: Vec<Dhcpv6Option>,
}

#[derive(Debug, Clone)]
pub struct Dhcpv6Option {
    pub code: u16,
    pub data: Vec<u8>,
}

impl Dhcpv6Packet {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 4 {
            return None;
        }

        let msg_type = data[0];
        let transaction_id = [data[1], data[2], data[3]];

        let options = parse_v6_options(&data[4..])?;

        Some(Self {
            msg_type,
            transaction_id,
            options,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(self.msg_type);
        buf.extend_from_slice(&self.transaction_id);

        for opt in &self.options {
            buf.extend_from_slice(&opt.code.to_be_bytes());
            buf.extend_from_slice(&(opt.data.len() as u16).to_be_bytes());
            buf.extend_from_slice(&opt.data);
        }

        buf
    }

    pub fn message_type(&self) -> Option<Dhcpv6MessageType> {
        Dhcpv6MessageType::from_u8(self.msg_type)
    }

    pub fn get_option(&self, code: u16) -> Option<&Dhcpv6Option> {
        self.options.iter().find(|o| o.code == code)
    }

    /// Extract client DUID from Client ID option.
    pub fn client_id(&self) -> Option<Vec<u8>> {
        self.get_option(OPT_CLIENTID).map(|o| o.data.clone())
    }
}

fn parse_v6_options(data: &[u8]) -> Option<Vec<Dhcpv6Option>> {
    let mut options = Vec::new();
    let mut i = 0;

    while i + 4 <= data.len() {
        let code = u16::from_be_bytes([data[i], data[i + 1]]);
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        i += 4;

        if i + len > data.len() {
            break;
        }

        options.push(Dhcpv6Option {
            code,
            data: data[i..i + len].to_vec(),
        });
        i += len;
    }

    Some(options)
}

/// Build a Server ID option with a DUID-LLT.
pub fn build_server_id(mac: &[u8; 6]) -> Dhcpv6Option {
    let mut data = Vec::new();
    // DUID-LL (type 3)
    data.extend_from_slice(&3u16.to_be_bytes());
    // Hardware type: Ethernet (1)
    data.extend_from_slice(&1u16.to_be_bytes());
    data.extend_from_slice(mac);

    Dhcpv6Option {
        code: OPT_SERVERID,
        data,
    }
}

/// Build a DNS recursive name server option.
pub fn build_dns_option(servers: &[Ipv6Addr]) -> Dhcpv6Option {
    let mut data = Vec::new();
    for s in servers {
        data.extend_from_slice(&s.octets());
    }
    Dhcpv6Option {
        code: OPT_DNS_SERVERS,
        data,
    }
}

/// Build an IA_NA option with an address.
pub fn build_ia_na(iaid: u32, addr: Ipv6Addr, preferred: u32, valid: u32) -> Dhcpv6Option {
    let mut data = Vec::new();
    data.extend_from_slice(&iaid.to_be_bytes());
    data.extend_from_slice(&0u32.to_be_bytes()); // T1
    data.extend_from_slice(&0u32.to_be_bytes()); // T2

    // Nested IA Address option
    let mut ia_addr = Vec::new();
    ia_addr.extend_from_slice(&addr.octets());
    ia_addr.extend_from_slice(&preferred.to_be_bytes());
    ia_addr.extend_from_slice(&valid.to_be_bytes());

    // IA Address option header
    data.extend_from_slice(&OPT_IAADDR.to_be_bytes());
    data.extend_from_slice(&(ia_addr.len() as u16).to_be_bytes());
    data.extend_from_slice(&ia_addr);

    Dhcpv6Option {
        code: OPT_IA_NA,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_v6_roundtrip() {
        let packet = Dhcpv6Packet {
            msg_type: 1, // Solicit
            transaction_id: [0x12, 0x34, 0x56],
            options: vec![Dhcpv6Option {
                code: OPT_CLIENTID,
                data: vec![0, 1, 0, 1, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff],
            }],
        };

        let bytes = packet.to_bytes();
        let parsed = Dhcpv6Packet::parse(&bytes).unwrap();

        assert_eq!(parsed.msg_type, 1);
        assert_eq!(parsed.transaction_id, [0x12, 0x34, 0x56]);
        assert!(parsed.client_id().is_some());
    }
}
