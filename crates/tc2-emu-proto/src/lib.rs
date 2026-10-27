//! Messages between the stubs running in the guest and the device models in
//! `tc2-emu-hw`, sent as one JSON object per line over a virtio-serial port.

use serde::{Deserialize, Serialize};

/// virtio-serial port name; appears in the guest as `/dev/virtio-ports/<name>`.
pub const PORT_NAME: &str = "org.cacophony.tc2-emu.hw";

/// One I2C transaction, as made through `org.cacophony.i2c.Tx`: write `write`
/// to `address`, then read `read_len` bytes back.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Request {
    pub id: u64,
    pub address: u8,
    pub write: Vec<u8>,
    pub read_len: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Response {
    pub id: u64,
    pub result: Result<Vec<u8>, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_wire_format() {
        let req = Request { id: 7, address: 0x25, write: vec![0x00], read_len: 1 };
        assert_eq!(
            serde_json::to_string(&req).unwrap(),
            r#"{"id":7,"address":37,"write":[0],"read_len":1}"#
        );
    }
}
