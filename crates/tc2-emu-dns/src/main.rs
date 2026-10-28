//! Fake DNS for the guest, in place of dnsmasq's. The guest can't reach the
//! internet, but services wait for modemd's connection check, which resolves
//! google.com (see scripts/guest-setup.sh). Every name resolves to the host side
//! of QEMU's network, where only the test API is reachable, and every other
//! record type, AAAA included, gets an immediate empty answer. dnsmasq forwards
//! what it can't answer to the image's upstream servers, which never reply, so
//! each AAAA lookup, and so each connection check, took 20s.

use anyhow::Context;
use clap::Parser;
use log::{debug, info, warn};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

const HEADER_LEN: usize = 12;
const TYPE_A: u16 = 1;
const TYPE_ANY: u16 = 255;
const CLASS_IN: u16 = 1;
const RCODE_FORMERR: u16 = 1;
const RCODE_NOTIMP: u16 = 4;

#[derive(Parser)]
struct Args {
    /// Address every name resolves to.
    #[arg(long, default_value = "192.168.1.1")]
    address: Ipv4Addr,
    /// Where to listen. `[::]` takes IPv4 queries too.
    #[arg(long, default_value = "[::]:53")]
    listen: SocketAddr,
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();
    let socket =
        UdpSocket::bind(args.listen).with_context(|| format!("binding {}", args.listen))?;
    info!("Resolving every name to {} on {}", args.address, args.listen);
    let mut buf = [0; 4096];
    loop {
        let (len, from) = match socket.recv_from(&mut buf) {
            Ok(received) => received,
            Err(e) => {
                warn!("Receiving: {e}");
                continue;
            }
        };
        let Some(reply) = respond(&buf[..len], args.address) else {
            debug!("Ignoring a malformed query from {from}");
            continue;
        };
        if let Err(e) = socket.send_to(&reply, from) {
            warn!("Replying to {from}: {e}");
        }
    }
}

/// The reply to `query`: an A record for `address` for an A (or ANY) query,
/// otherwise no records. `None` if it isn't a query at all.
fn respond(query: &[u8], address: Ipv4Addr) -> Option<Vec<u8>> {
    if query.len() < HEADER_LEN {
        return None;
    }
    let query_flags = u16::from_be_bytes([query[2], query[3]]);
    if query_flags & 0x8000 != 0 {
        return None;
    }
    let opcode = (query_flags >> 11) & 0xf;
    let header = |rcode: u16, qdcount: u16, ancount: u16| {
        // QR, the query's opcode, AA, RD as asked, RA.
        let flags = 0x8000 | (opcode << 11) | 0x0400 | (query_flags & 0x0100) | 0x0080 | rcode;
        let mut header = query[..2].to_vec();
        for field in [flags, qdcount, ancount, 0, 0] {
            header.extend_from_slice(&field.to_be_bytes());
        }
        header
    };
    if opcode != 0 {
        return Some(header(RCODE_NOTIMP, 0, 0));
    }
    let qdcount = u16::from_be_bytes([query[4], query[5]]);
    let Some(end) = (qdcount == 1).then(|| question_end(query)).flatten() else {
        return Some(header(RCODE_FORMERR, 0, 0));
    };
    let question = &query[HEADER_LEN..end];
    let qtype = u16::from_be_bytes([question[question.len() - 4], question[question.len() - 3]]);
    let qclass = u16::from_be_bytes([question[question.len() - 2], question[question.len() - 1]]);
    let answer = qclass == CLASS_IN && matches!(qtype, TYPE_A | TYPE_ANY);
    debug!("{} type {qtype}: {}", name(question), if answer { "answered" } else { "no records" });

    let mut reply = header(0, 1, answer.into());
    reply.extend_from_slice(question);
    if answer {
        // The question's name, by a pointer to it.
        reply.extend_from_slice(&(0xc000 | HEADER_LEN as u16).to_be_bytes());
        reply.extend_from_slice(&TYPE_A.to_be_bytes());
        reply.extend_from_slice(&CLASS_IN.to_be_bytes());
        // TTL 0, so nothing caches it.
        reply.extend_from_slice(&0u32.to_be_bytes());
        reply.extend_from_slice(&4u16.to_be_bytes());
        reply.extend_from_slice(&address.octets());
    }
    Some(reply)
}

/// The end of the question in `query`: a name, uncompressed as in any query,
/// then its type and class.
fn question_end(query: &[u8]) -> Option<usize> {
    let mut pos = HEADER_LEN;
    loop {
        let len = *query.get(pos)? as usize;
        pos += 1 + len;
        if len == 0 {
            break;
        }
        // Compression pointers or other label types, or longer than a name can be.
        if len > 63 || pos - HEADER_LEN > 255 {
            return None;
        }
    }
    (pos + 4 <= query.len()).then_some(pos + 4)
}

/// The name a question (from `question_end`) asks about, for logging.
fn name(question: &[u8]) -> String {
    let mut labels = Vec::new();
    let mut pos = 0;
    while question[pos] != 0 {
        let len = question[pos] as usize;
        labels.push(String::from_utf8_lossy(&question[pos + 1..pos + 1 + len]));
        pos += 1 + len;
    }
    labels.join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDRESS: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 1);

    /// A query for `name` and `qtype`, as a stub resolver sends it: id 0x1234,
    /// recursion desired.
    fn query(name: &str, qtype: u16) -> Vec<u8> {
        let mut query = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
        for label in name.split('.') {
            query.push(label.len() as u8);
            query.extend_from_slice(label.as_bytes());
        }
        query.push(0);
        query.extend_from_slice(&qtype.to_be_bytes());
        query.extend_from_slice(&CLASS_IN.to_be_bytes());
        query
    }

    fn field(reply: &[u8], index: usize) -> u16 {
        u16::from_be_bytes([reply[index], reply[index + 1]])
    }

    #[test]
    fn answers_a_queries_with_the_address() {
        let query = query("google.com", TYPE_A);
        let reply = respond(&query, ADDRESS).unwrap();
        assert_eq!(&reply[..2], &[0x12, 0x34]);
        // QR, AA, RD, RA, NOERROR; one question, one answer.
        assert_eq!(field(&reply, 2), 0x8580);
        assert_eq!([field(&reply, 4), field(&reply, 6)], [1, 1]);
        assert_eq!(&reply[HEADER_LEN..query.len()], &query[HEADER_LEN..]);
        assert_eq!(
            &reply[query.len()..],
            &[0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 0, 0, 4, 192, 168, 1, 1]
        );
    }

    #[test]
    fn answers_other_types_with_no_records() {
        for qtype in [28 /* AAAA */, 15 /* MX */, 12 /* PTR */] {
            let query = query("google.com", qtype);
            let reply = respond(&query, ADDRESS).unwrap();
            assert_eq!(field(&reply, 2), 0x8580, "type {qtype}");
            assert_eq!([field(&reply, 4), field(&reply, 6)], [1, 0], "type {qtype}");
            assert_eq!(&reply[HEADER_LEN..], &query[HEADER_LEN..], "type {qtype}");
        }
    }

    #[test]
    fn ignores_trailing_edns_record() {
        let mut query = query("google.com", TYPE_A);
        query[11] = 1;
        query.extend_from_slice(&[0, 0, 41, 0x10, 0, 0, 0, 0, 0, 0, 0]);
        let reply = respond(&query, ADDRESS).unwrap();
        assert_eq!([field(&reply, 4), field(&reply, 6), field(&reply, 10)], [1, 1, 0]);
        assert_eq!(&reply[reply.len() - 4..], &[192, 168, 1, 1]);
    }

    #[test]
    fn rejects_what_it_cannot_parse() {
        // Two questions.
        let mut two = query("google.com", TYPE_A);
        two[5] = 2;
        // A compression pointer in place of a name.
        let mut pointer = query("google.com", TYPE_A)[..HEADER_LEN].to_vec();
        pointer.extend_from_slice(&[0xc0, 12, 0, 1, 0, 1]);
        // Cut off before the type and class.
        let truncated = query("google.com", TYPE_A)[..HEADER_LEN + 12].to_vec();
        for query in [two, pointer, truncated] {
            let reply = respond(&query, ADDRESS).unwrap();
            assert_eq!(reply.len(), HEADER_LEN);
            assert_eq!(field(&reply, 2) & 0xf, RCODE_FORMERR);
        }

        let mut status = query("google.com", TYPE_A);
        status[2] |= 2 << 3;
        assert_eq!(field(&respond(&status, ADDRESS).unwrap(), 2) & 0xf, RCODE_NOTIMP);
    }

    #[test]
    fn ignores_responses_and_runts() {
        let mut response = query("google.com", TYPE_A);
        response[2] |= 0x80;
        assert_eq!(respond(&response, ADDRESS), None);
        assert_eq!(respond(&[0x12, 0x34, 1], ADDRESS), None);
    }
}
