use super::services;
use crate::cell::Cell;
use crate::fmt;
use crate::io::{self, IoSlice, IoSliceMut};
use crate::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4, SocketAddrV6};
use crate::sys::unsupported;
use crate::time::Duration;

mod dns;

macro_rules! unimpl {
    () => {
        return Err(io::Error::new_const(
            io::ErrorKind::Unsupported,
            &"This function is not yet implemented",
        ));
    };
}

pub struct TcpStream {
    fd: usize,
    local_port: u16,
    remote_port: u16,
    peer_addr: SocketAddr,
    // milliseconds
    read_timeout: Cell<u32>,
    // milliseconds
    write_timeout: Cell<u32>,
}

#[repr(C, align(4096))]
struct ConnectRequest {
    raw: [u8; 4096],
}

#[repr(C, align(4096))]
struct SendData {
    raw: [u8; 4096],
}

#[repr(C, align(4096))]
pub struct ReceiveData {
    raw: [u8; 4096],
}

#[repr(C, align(4096))]
pub struct GetAddress {
    raw: [u8; 4096],
}

impl TcpStream {
    pub fn connect(socketaddr: io::Result<&SocketAddr>) -> io::Result<TcpStream> {
        Self::connect_timeout(socketaddr?, Duration::ZERO)
    }

    pub fn connect_timeout(addr: &SocketAddr, duration: Duration) -> io::Result<TcpStream> {
        let mut connect_request = ConnectRequest { raw: [0u8; 4096] };

        // Construct the request.
        let port_bytes = addr.port().to_le_bytes();
        connect_request.raw[0] = port_bytes[0];
        connect_request.raw[1] = port_bytes[1];
        for (dest, src) in
            connect_request.raw[2..].iter_mut().zip((duration.as_millis() as u64).to_le_bytes())
        {
            *dest = src;
        }
        match addr.ip() {
            IpAddr::V4(addr) => {
                connect_request.raw[10] = 4;
                for (dest, src) in connect_request.raw[11..].iter_mut().zip(addr.octets()) {
                    *dest = src;
                }
            }
            IpAddr::V6(addr) => {
                connect_request.raw[10] = 6;
                for (dest, src) in connect_request.raw[11..].iter_mut().zip(addr.octets()) {
                    *dest = src;
                }
            }
        }

        let buf = unsafe {
            xous::MemoryRange::new(
                &mut connect_request as *mut ConnectRequest as usize,
                core::mem::size_of::<ConnectRequest>(),
            )
            .unwrap()
        };

        let response = xous::send_message(
            services::network(),
            xous::Message::new_lend_mut(
                30, /* StdTcpConnect */
                buf,
                None,
                xous::MemorySize::new(4096),
            ),
        );

        if let Ok(xous::Result::MemoryReturned(_, valid)) = response {
            // The first four bytes should be zero upon success, and will be nonzero
            // for an error.
            let response = buf.as_slice::<u16>();
            if response[0] != 0 || valid.is_none() {
                // TODO: generate the correct error here
                return Err(io::Error::new_const(
                    io::ErrorKind::InvalidInput,
                    &"Unable to connect",
                ));
            }
            let fd = response[1] as usize;
            let local_port = response[2];
            let remote_port = response[3];
            // println!(
            //     "Connected with local port of {}, remote port of {}, file handle of {}",
            //     local_port, remote_port, fd
            // );
            return Ok(TcpStream {
                fd,
                local_port,
                remote_port,
                peer_addr: *addr,
                read_timeout: Cell::new(0),
                write_timeout: Cell::new(0),
            });
        }
        Err(io::Error::new_const(io::ErrorKind::InvalidInput, &"Invalid response"))
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.read_timeout
            .set(timeout.map(|t| t.as_millis().min(u32::MAX as u128) as u32).unwrap_or_default());
        Ok(())
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.write_timeout
            .set(timeout.map(|t| t.as_millis().min(u32::MAX as u128) as u32).unwrap_or_default());
        Ok(())
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        match self.read_timeout.get() {
            0 => Ok(None),
            t => Ok(Some(Duration::from_millis(t as u64))),
        }
    }

    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        match self.write_timeout.get() {
            0 => Ok(None),
            t => Ok(Some(Duration::from_millis(t as u64))),
        }
    }

    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut receive_request = ReceiveData { raw: [0u8; 4096] };
        let data_to_read = buf.len().min(receive_request.raw.len());

        let range = unsafe {
            xous::MemoryRange::new(&mut receive_request as *mut ReceiveData as usize, 4096).unwrap()
        };

        if let Ok(xous::Result::MemoryReturned(_offset, valid)) = xous::send_message(
            services::network(),
            xous::Message::new_lend_mut(
                33 | (self.fd << 16), /* StdTcpRx */
                range,
                None,
                xous::MemorySize::new(data_to_read),
            ),
        ) {
            // println!("offset: {:?}, valid: {:?}", offset, valid);
            if let Some(length) = valid {
                let length = length.get();
                for (dest, src) in buf.iter_mut().zip(receive_request.raw[..length].iter()) {
                    *dest = *src;
                }
                Ok(length)
            } else {
                Ok(0)
            }
        } else {
            Err(io::Error::new_const(io::ErrorKind::InvalidInput, &"Unable to peek"))
        }
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut receive_request = ReceiveData { raw: [0u8; 4096] };
        let data_to_read = buf.len().min(receive_request.raw.len());

        let range = unsafe {
            xous::MemoryRange::new(&mut receive_request as *mut ReceiveData as usize, 4096).unwrap()
        };

        if let Ok(xous::Result::MemoryReturned(_offset, valid)) = xous::send_message(
            services::network(),
            xous::Message::new_lend_mut(
                33 | (self.fd << 16), /* StdTcpRx */
                range,
                // Reuse the `offset` as the read timeout
                xous::MemoryAddress::new(self.read_timeout.get() as usize),
                xous::MemorySize::new(data_to_read),
            ),
        ) {
            // println!("offset: {:?}, valid: {:?}", offset, valid);
            if let Some(length) = valid {
                let length = length.get();
                for (dest, src) in buf.iter_mut().zip(receive_request.raw[..length].iter()) {
                    *dest = *src;
                }
                Ok(length)
            } else {
                Ok(0)
            }
        } else {
            Err(io::Error::new_const(io::ErrorKind::InvalidInput, &"Unable to read"))
        }
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        crate::io::default_read_vectored(|b| self.read(b), bufs)
    }

    pub fn is_read_vectored(&self) -> bool {
        false
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        let mut send_request = SendData { raw: [0u8; 4096] };
        for (dest, src) in send_request.raw.iter_mut().zip(buf) {
            *dest = *src;
        }

        let range = unsafe {
            xous::MemoryRange::new(
                &mut send_request as *mut SendData as usize,
                core::mem::size_of::<SendData>(),
            )
            .unwrap()
        };

        let response = xous::send_message(
            services::network(),
            xous::Message::new_lend_mut(
                31 | (self.fd << 16), /* StdTcpTx */
                range,
                // Reuse the offset as the timeout
                xous::MemoryAddress::new(self.write_timeout.get() as usize),
                xous::MemorySize::new(buf.len().min(send_request.raw.len())),
            ),
        )
        .or(Err(io::Error::new_const(io::ErrorKind::InvalidInput, &"Internal error")))?;

        if let xous::Result::MemoryReturned(_offset, _valid) = response {
            let result = range.as_slice::<u32>();
            if result[0] != 0 {
                // println!("Error in sending: {}", result[1]);
                return Err(io::Error::new_const(
                    io::ErrorKind::InvalidInput,
                    &"Error when sending",
                ));
            }
            Ok(result[1] as usize)
        } else {
            Err(io::Error::new_const(io::ErrorKind::InvalidInput, &"Unexpected return value"))
        }
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        crate::io::default_write_vectored(|b| self.write(b), bufs)
    }

    pub fn is_write_vectored(&self) -> bool {
        false
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer_addr)
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        let mut get_addr = GetAddress { raw: [0u8; 4096] };
        let range = unsafe {
            xous::MemoryRange::new(
                &mut get_addr as *mut GetAddress as usize,
                core::mem::size_of::<GetAddress>(),
            )
            .unwrap()
        };

        match xous::send_message(
            services::network(),
            xous::Message::new_lend_mut(
                35 | (self.fd << 16), /* StdGetAddress */
                range,
                None,
                None,
            ),
        ) {
            Ok(xous::Result::MemoryReturned(_offset, _valid)) => {
                let mut i = get_addr.raw.iter();
                match *i.next().unwrap() {
                    4 => Ok(SocketAddr::V4(SocketAddrV4::new(
                        Ipv4Addr::new(
                            *i.next().unwrap(),
                            *i.next().unwrap(),
                            *i.next().unwrap(),
                            *i.next().unwrap(),
                        ),
                        self.local_port,
                    ))),
                    6 => {
                        let mut new_addr = [0u8; 16];
                        for (src, octet) in i.zip(new_addr.iter_mut()) {
                            *octet = *src;
                        }
                        Ok(SocketAddr::V6(SocketAddrV6::new(
                            new_addr.into(),
                            self.local_port,
                            0,
                            0,
                        )))
                    }
                    _ => Err(io::Error::new_const(io::ErrorKind::InvalidInput, &"Internal error")),
                }
            }
            _ => Err(io::Error::new_const(io::ErrorKind::InvalidInput, &"Internal error")),
        }
    }

    pub fn shutdown(&self, _: Shutdown) -> io::Result<()> {
        xous::send_message(
            self.fd as _,
            xous::Message::new_blocking_scalar(34 | ((self.fd as usize) << 16), 0, 0, 0, 0),
        )
        .or(Err(io::Error::new_const(io::ErrorKind::InvalidInput, &"Unexpected return value")))
        .map(|_| ())
    }

    pub fn duplicate(&self) -> io::Result<TcpStream> {
        unimpl!();
    }

    pub fn set_linger(&self, _: Option<Duration>) -> io::Result<()> {
        unimpl!();
    }

    pub fn linger(&self) -> io::Result<Option<Duration>> {
        unimpl!();
    }

    pub fn set_nodelay(&self, enabled: bool) -> io::Result<()> {
        xous::send_message(
            self.fd as _,
            xous::Message::new_blocking_scalar(
                39 | ((self.fd as usize) << 16), //StdSetNodelay = 39
                if enabled { 1 } else { 0 },
                0,
                0,
                0,
            ),
        )
        .or(Err(io::Error::new_const(io::ErrorKind::InvalidInput, &"Unexpected return value")))
        .map(|_| ())
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        let result = xous::send_message(
            self.fd as _,
            xous::Message::new_blocking_scalar(
                38 | ((self.fd as usize) << 16), //StdGetNodelay = 38
                0,
                0,
                0,
                0,
            ),
        )
        .or(Err(io::Error::new_const(io::ErrorKind::InvalidInput, &"Unexpected return value")))?;
        if let xous::Result::Scalar1(enabled) = result {
            Ok(enabled != 0)
        } else {
            Err(io::Error::new_const(io::ErrorKind::InvalidInput, &"Unexpected return value"))
        }
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        xous::send_message(
            self.fd as _,
            xous::Message::new_blocking_scalar(
                37 | ((self.fd as usize) << 16), //StdSetTtl = 37
                ttl as usize,
                0,
                0,
                0,
            ),
        )
        .or(Err(io::Error::new_const(io::ErrorKind::InvalidInput, &"Unexpected return value")))
        .map(|_| ())
    }

    pub fn ttl(&self) -> io::Result<u32> {
        xous::send_message(
            self.fd as _,
            xous::Message::new_blocking_scalar(
                38 | ((self.fd as usize) << 16), //StdGetNodelay = 38
                0,
                0,
                0,
                0,
            ),
        )
        .or(Err(io::Error::new_const(io::ErrorKind::InvalidInput, &"Unexpected return value")))
        .and_then(|res| {
            if let xous::Result::Scalar1(ttl) = res {
                Ok(ttl as u32)
            } else {
                Err(io::Error::new_const(io::ErrorKind::InvalidInput, &"Unexpected return value"))
            }
        })
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        unimpl!();
    }

    pub fn set_nonblocking(&self, _: bool) -> io::Result<()> {
        unimpl!();
    }
}

impl fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Network connection to {:?} port {} to local port {}",
            self.peer_addr, self.remote_port, self.local_port
        )
    }
}

pub struct TcpListener(!);

impl TcpListener {
    pub fn bind(_: io::Result<&SocketAddr>) -> io::Result<TcpListener> {
        unsupported()
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        self.0
    }

    pub fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        self.0
    }

    pub fn duplicate(&self) -> io::Result<TcpListener> {
        self.0
    }

    pub fn set_ttl(&self, _: u32) -> io::Result<()> {
        unimpl!();
    }

    pub fn ttl(&self) -> io::Result<u32> {
        unimpl!();
    }

    pub fn set_only_v6(&self, _: bool) -> io::Result<()> {
        unimpl!();
    }

    pub fn only_v6(&self) -> io::Result<bool> {
        unimpl!();
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        unimpl!();
    }

    pub fn set_nonblocking(&self, _: bool) -> io::Result<()> {
        unimpl!();
    }
}

impl fmt::Debug for TcpListener {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
    }
}

pub struct UdpSocket(!);

impl UdpSocket {
    pub fn bind(_: io::Result<&SocketAddr>) -> io::Result<UdpSocket> {
        unsupported()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.0
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        self.0
    }

    pub fn recv_from(&self, _: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.0
    }

    pub fn peek_from(&self, _: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.0
    }

    pub fn send_to(&self, _: &[u8], _: &SocketAddr) -> io::Result<usize> {
        self.0
    }

    pub fn duplicate(&self) -> io::Result<UdpSocket> {
        self.0
    }

    pub fn set_read_timeout(&self, _: Option<Duration>) -> io::Result<()> {
        self.0
    }

    pub fn set_write_timeout(&self, _: Option<Duration>) -> io::Result<()> {
        self.0
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        self.0
    }

    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        self.0
    }

    pub fn set_broadcast(&self, _: bool) -> io::Result<()> {
        self.0
    }

    pub fn broadcast(&self) -> io::Result<bool> {
        self.0
    }

    pub fn set_multicast_loop_v4(&self, _: bool) -> io::Result<()> {
        self.0
    }

    pub fn multicast_loop_v4(&self) -> io::Result<bool> {
        self.0
    }

    pub fn set_multicast_ttl_v4(&self, _: u32) -> io::Result<()> {
        self.0
    }

    pub fn multicast_ttl_v4(&self) -> io::Result<u32> {
        self.0
    }

    pub fn set_multicast_loop_v6(&self, _: bool) -> io::Result<()> {
        self.0
    }

    pub fn multicast_loop_v6(&self) -> io::Result<bool> {
        self.0
    }

    pub fn join_multicast_v4(&self, _: &Ipv4Addr, _: &Ipv4Addr) -> io::Result<()> {
        self.0
    }

    pub fn join_multicast_v6(&self, _: &Ipv6Addr, _: u32) -> io::Result<()> {
        self.0
    }

    pub fn leave_multicast_v4(&self, _: &Ipv4Addr, _: &Ipv4Addr) -> io::Result<()> {
        self.0
    }

    pub fn leave_multicast_v6(&self, _: &Ipv6Addr, _: u32) -> io::Result<()> {
        self.0
    }

    pub fn set_ttl(&self, _: u32) -> io::Result<()> {
        self.0
    }

    pub fn ttl(&self) -> io::Result<u32> {
        self.0
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.0
    }

    pub fn set_nonblocking(&self, _: bool) -> io::Result<()> {
        self.0
    }

    pub fn recv(&self, _: &mut [u8]) -> io::Result<usize> {
        self.0
    }

    pub fn peek(&self, _: &mut [u8]) -> io::Result<usize> {
        self.0
    }

    pub fn send(&self, _: &[u8]) -> io::Result<usize> {
        self.0
    }

    pub fn connect(&self, _: io::Result<&SocketAddr>) -> io::Result<()> {
        self.0
    }
}

impl fmt::Debug for UdpSocket {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
    }
}

pub use dns::LookupHost;

#[allow(nonstandard_style)]
pub mod netc {
    pub const AF_INET: u8 = 0;
    pub const AF_INET6: u8 = 1;
    pub type sa_family_t = u8;

    #[derive(Copy, Clone)]
    pub struct in_addr {
        pub s_addr: u32,
    }

    #[derive(Copy, Clone)]
    pub struct sockaddr_in {
        pub sin_family: sa_family_t,
        pub sin_port: u16,
        pub sin_addr: in_addr,
    }

    #[derive(Copy, Clone)]
    pub struct in6_addr {
        pub s6_addr: [u8; 16],
    }

    #[derive(Copy, Clone)]
    pub struct sockaddr_in6 {
        pub sin6_family: sa_family_t,
        pub sin6_port: u16,
        pub sin6_addr: in6_addr,
        pub sin6_flowinfo: u32,
        pub sin6_scope_id: u32,
    }

    #[derive(Copy, Clone)]
    pub struct sockaddr {}

    pub type socklen_t = usize;
}
