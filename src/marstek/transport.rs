//! Bounded UDP exchanges for read-only device requests.
use super::protocol::{ReadRequest, ResponseError, validate_response};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum TransportError {
    Io(io::Error),
    Serialization(serde_json::Error),
    Invalid(ResponseError),
    Timeout,
}
impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "network error: {error}"),
            Self::Serialization(error) => write!(f, "serialization error: {error}"),
            Self::Invalid(error) => write!(f, "invalid response: {error:?}"),
            Self::Timeout => write!(f, "timeout"),
        }
    }
}
impl From<io::Error> for TransportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<serde_json::Error> for TransportError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

pub struct Transport {
    socket: UdpSocket,
    next_id: u64,
}
impl Transport {
    pub fn bind() -> Result<Self, TransportError> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        socket.set_broadcast(true)?;
        Ok(Self { socket, next_id: 1 })
    }
    pub fn last_request_id(&self) -> u64 {
        self.next_id - 1
    }
    fn send(&mut self, endpoint: SocketAddr, request: ReadRequest) -> Result<u64, TransportError> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("request ID exhausted"))?;
        self.socket.send_to(&request.serialize(id)?, endpoint)?;
        Ok(id)
    }
    fn receive(&self, deadline: Instant) -> Result<(Vec<u8>, SocketAddr), TransportError> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(TransportError::Timeout)?;
        if remaining.is_zero() {
            return Err(TransportError::Timeout);
        }
        self.socket.set_read_timeout(Some(remaining))?;
        let mut buffer = vec![0; 65536];
        let (length, sender) = match self.socket.recv_from(&mut buffer) {
            Ok(value) => value,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(TransportError::Timeout);
            }
            Err(error) => return Err(error.into()),
        };
        if Instant::now() >= deadline {
            return Err(TransportError::Timeout);
        }
        buffer.truncate(length);
        Ok((buffer, sender))
    }
    pub fn exchange(
        &mut self,
        endpoint: SocketAddr,
        request: ReadRequest,
        identity: Option<&str>,
    ) -> Result<serde_json::Value, TransportError> {
        let id = self.send(endpoint, request)?;
        let deadline = Instant::now() + DEADLINE;
        loop {
            let (bytes, sender) = self.receive(deadline)?;
            if sender != endpoint {
                continue;
            }
            match validate_response(&bytes, id, request, identity) {
                Ok(response) => return Ok(response),
                Err(ResponseError::WrongRequestId) => continue,
                Err(error) => return Err(TransportError::Invalid(error)),
            }
        }
    }
    pub fn discover(
        &mut self,
        port: u16,
    ) -> Result<Vec<(SocketAddr, serde_json::Value)>, TransportError> {
        let endpoint = SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), port);
        let id = self.send(endpoint, ReadRequest::Device)?;
        let deadline = Instant::now() + DEADLINE;
        let mut candidates = Vec::new();
        loop {
            let (bytes, sender) = match self.receive(deadline) {
                Ok(packet) => packet,
                Err(TransportError::Timeout) => break,
                Err(TransportError::Invalid(_)) => continue,
                Err(error) => return Err(error),
            };
            if sender.port() != port || candidates.iter().any(|(address, _)| *address == sender) {
                continue;
            }
            if let Ok(response) = validate_response(&bytes, id, ReadRequest::Device, None) {
                candidates.push((sender, response));
                if candidates.len() == 16 {
                    break;
                }
            }
        }
        Ok(candidates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    fn responder(payload: Vec<u8>) -> (SocketAddr, thread::JoinHandle<()>) {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = socket.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut buffer = [0; 2048];
            let (_, peer) = socket.recv_from(&mut buffer).unwrap();
            socket.send_to(&payload, peer).unwrap();
        });
        (address, handle)
    }
    #[test]
    fn accepts_correlated_response() {
        let (address, handle) =
            responder(br#"{"id":1,"src":"synthetic","result":{"id":0,"soc":0}}"#.to_vec());
        let result =
            Transport::bind()
                .unwrap()
                .exchange(address, ReadRequest::Battery, Some("synthetic"));
        assert!(result.is_ok());
        handle.join().unwrap();
    }
    #[test]
    fn rejects_malformed_and_oversized_packets() {
        for payload in [b"{".to_vec(), vec![b'x'; 16385]] {
            let (address, handle) = responder(payload);
            assert!(matches!(
                Transport::bind().unwrap().exchange(
                    address,
                    ReadRequest::Battery,
                    Some("synthetic")
                ),
                Err(TransportError::Invalid(_))
            ));
            handle.join().unwrap();
        }
    }
    #[test]
    fn ignores_oversized_packet_from_unrelated_sender() {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = socket.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut buffer = [0; 2048];
            let (_, peer) = socket.recv_from(&mut buffer).unwrap();
            let unrelated = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            unrelated.send_to(&vec![b'x'; 16385], peer).unwrap();
            socket
                .send_to(
                    br#"{"id":1,"src":"synthetic","result":{"id":0,"soc":50}}"#,
                    peer,
                )
                .unwrap();
        });
        let start = Instant::now();
        let response = Transport::bind()
            .unwrap()
            .exchange(address, ReadRequest::Battery, Some("synthetic"))
            .unwrap();
        assert_eq!(response["result"]["soc"], 50);
        assert!(start.elapsed() < DEADLINE);
        handle.join().unwrap();
    }

    #[test]
    fn rejects_wrong_id_and_wrong_sender_until_timeout() {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = socket.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut buffer = [0; 2048];
            let (_, peer) = socket.recv_from(&mut buffer).unwrap();
            let other = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            other
                .send_to(br#"{"id":1,"src":"synthetic","result":{"id":0}}"#, peer)
                .unwrap();
            socket
                .send_to(br#"{"id":99,"src":"synthetic","result":{"id":0}}"#, peer)
                .unwrap();
        });
        let start = Instant::now();
        assert!(matches!(
            Transport::bind()
                .unwrap()
                .exchange(address, ReadRequest::Battery, Some("synthetic")),
            Err(TransportError::Timeout)
        ));
        assert!(start.elapsed() < Duration::from_secs(3));
        handle.join().unwrap();
    }
    #[test]
    fn late_response_cannot_satisfy_next_request() {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = socket.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut buffer = [0; 2048];
            let (_, peer) = socket.recv_from(&mut buffer).unwrap();
            thread::sleep(Duration::from_millis(2100));
            socket
                .send_to(br#"{"id":1,"src":"synthetic","result":{"id":0}}"#, peer)
                .unwrap();
            let (_, peer) = socket.recv_from(&mut buffer).unwrap();
            socket
                .send_to(br#"{"id":2,"src":"synthetic","result":{"id":0}}"#, peer)
                .unwrap();
        });
        let mut transport = Transport::bind().unwrap();
        assert!(matches!(
            transport.exchange(address, ReadRequest::Battery, Some("synthetic")),
            Err(TransportError::Timeout)
        ));
        let response = transport
            .exchange(address, ReadRequest::Battery, Some("synthetic"))
            .unwrap();
        assert_eq!(response["id"], 2);
        handle.join().unwrap();
    }
}
