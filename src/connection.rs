//use std::io;
use mio::{Token, Ready};
use mio::unix::UnixReady;
use mio::tcp::TcpStream;
use std::io::prelude::*;
use std::io::ErrorKind;
use std::ptr;

#[derive(Debug, Copy, Clone)]
pub enum TokenType {
    Listener(ListenerToken),
    Incoming(IncomingToken),
    Outgoing(OutgoingToken),
}

#[derive(PartialEq, Eq, Hash, Debug, Copy, Clone)]
pub struct ListenerToken(pub usize);

#[derive(PartialEq, Eq, Hash, Debug, Copy, Clone)]
pub struct IncomingToken(pub usize);

#[derive(PartialEq, Eq, Hash, Debug, Copy, Clone)]
pub struct OutgoingToken(pub usize);

type BufferArray = [u8; 4096];

#[derive(Copy, Clone)]
pub enum EndPointType {
    Front = 0,
    Back = 1,
}

pub struct EndPoint {
    state: Ready,
    stream: TcpStream,
    buffer: BufferArray,
    buffer_index: usize,
    peer_stream: Option<TcpStream>,
}

impl EndPoint {
    pub fn new(tcp_stream: TcpStream) -> EndPoint {
        EndPoint {
            state: Ready::empty(),
            stream: tcp_stream,
            buffer: [0; 4096],
            buffer_index: 0,
            peer_stream: None,
        }
    }

    pub fn set_peer_stream(&mut self, tcp_stream: &TcpStream) {
        if let Ok(stream) = tcp_stream.try_clone() {
            self.peer_stream = Some(stream);
        }
    }
    pub fn absorb(&mut self) -> usize {
        match self.stream
                  .read(self.buffer.split_at_mut(self.buffer_index).1) {
            Ok(n_read) => {
                self.buffer_index += n_read;
                return n_read;
            }
            Err(e) => {
                if e.kind() == ErrorKind::WouldBlock {
                    //                    info!("WouldBlock when read");
                    return 0;
                }
                error!("Reading caused error: {}", e);
            }
        }
        return 0;
    }

    pub fn pipe_to_peer(&mut self) -> usize {
        if self.buffer_index == 0 {
            return 0;
        }
        if let Some(mut dest) = self.peer_stream.as_mut() {
            match dest.write(self.buffer.split_at(self.buffer_index).0) {
                Ok(n_written) => {
                    let left = self.buffer_index - n_written;
                    if left > 0 {
                        unsafe {
                            ptr::copy(&self.buffer[n_written], &mut self.buffer[0], left);
                        }
                        info!("in shorten writeen");
                    }
                    self.buffer_index = left;
                    return n_written;
                }
                Err(e) => {
                    if e.kind() == ErrorKind::WouldBlock {
                        // info!("WouldBlock when read");
                        return 0;
                    }

                    error!("Reading caused error: {}", e);
                    return 0;
                }
            }
        }
        return 0;
    }
}

pub struct Connection {
    points: [EndPoint; 2],
    backend_token: OutgoingToken,
}

impl Connection {
    pub fn new(incoming_stream: TcpStream,
               outgoing_stream: TcpStream,
               outgoing_token: OutgoingToken)
               -> Connection {
        let mut front = EndPoint::new(incoming_stream);
        let mut backend = EndPoint::new(outgoing_stream);
        front.set_peer_stream(&backend.stream);
        backend.set_peer_stream(&front.stream);
        Connection {
            points: [front, backend],
            backend_token: outgoing_token,
        }
    }

    pub fn incoming_ready(&mut self, events: Ready) {
        self.points[EndPointType::Front as usize]
            .state
            .insert(events);
    }

    pub fn outgoing_ready(&mut self, events: Ready) {
        self.points[EndPointType::Back as usize]
            .state
            .insert(events);
    }

    pub fn is_outgoing_closed(&self) -> bool {
        let unix_ready = UnixReady::from(self.points[EndPointType::Back as usize].state);

        unix_ready.is_error() || unix_ready.is_hup()
    }

    pub fn is_incoming_closed(&self) -> bool {
        let unix_ready = UnixReady::from(self.points[EndPointType::Front as usize].state);

        unix_ready.is_error() || unix_ready.is_hup()
    }

    pub fn incoming_stream<'a>(&'a self) -> &'a TcpStream {
        &self.points[EndPointType::Front as usize].stream
    }

    pub fn outgoing_stream<'a>(&'a self) -> &'a TcpStream {
        &self.points[EndPointType::Back as usize].stream
    }

    pub fn outgoing_token(&self) -> OutgoingToken {
        self.backend_token
    }

    pub fn transfer(&mut self, src_index: usize, dest_index: usize) -> usize {
        let mut count = 0;
        if self.points[dest_index].state.is_writable() {
            count = self.points[src_index].pipe_to_peer();
            self.points[dest_index].state.remove(Ready::writable());
        }
        count
    }
    pub fn tick(&mut self) -> bool {
        let mut sended = false;
        for point in self.points.iter_mut() {
            if point.state.is_readable() && point.buffer_index < 4096 {
                point.absorb();
                point.state.remove(Ready::readable());
            }
        }

        sended |= self.transfer(EndPointType::Back as usize, EndPointType::Front as usize) > 0;
        sended |= self.transfer(EndPointType::Front as usize, EndPointType::Back as usize) > 0;
        sended
    }
}

impl TokenType {
    pub fn from_raw_token(t: Token) -> TokenType {
        let i = usize::from(t);

        match i & 3 {
            0 => TokenType::Listener(ListenerToken(i >> 2)),
            1 => TokenType::Incoming(IncomingToken(i >> 2)),
            2 => TokenType::Outgoing(OutgoingToken(i >> 2)),
            _ => unreachable!(),
        }
    }
}

impl ListenerToken {
    pub fn as_raw_token(self) -> Token {
        Token(self.0 << 2)
    }
}

impl IncomingToken {
    pub fn as_raw_token(self) -> Token {
        Token((self.0 << 2) + 1)
    }
}

impl OutgoingToken {
    pub fn as_raw_token(self) -> Token {
        Token((self.0 << 2) + 2)
    }
}

impl From<usize> for ListenerToken {
    fn from(i: usize) -> ListenerToken {
        ListenerToken(i)
    }
}

impl From<ListenerToken> for usize {
    fn from(val: ListenerToken) -> usize {
        val.0
    }
}

impl From<usize> for IncomingToken {
    fn from(i: usize) -> IncomingToken {
        IncomingToken(i)
    }
}

impl From<IncomingToken> for usize {
    fn from(val: IncomingToken) -> usize {
        val.0
    }
}

impl From<usize> for OutgoingToken {
    fn from(i: usize) -> OutgoingToken {
        OutgoingToken(i)
    }
}

impl From<OutgoingToken> for usize {
    fn from(val: OutgoingToken) -> usize {
        val.0
    }
}
