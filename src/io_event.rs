//! TODO

use std::io::{self, Read, Write};
#[cfg(windows)]
use std::net::SocketAddr;
use std::sync::atomic::{self, AtomicBool, Ordering};
use std::sync::Arc;

use socket2::{Domain, Socket, Type};

use crate::async_io::Async;

/// A self-pipe.
///
/// Explained in: https://cr.yp.to/docs/selfpipe.html
struct Inner {
    /// Set to `true` if notified.
    flag: AtomicBool,

    /// The writer side, emptied by `clear()`.
    writer: Socket,

    /// The reader side, filled by `notify()`.
    reader: Async<Socket>,
}

/// A flag that that triggers an I/O event whenever it is set.
#[derive(Clone)]
pub(crate) struct IoEvent(Arc<Inner>);

impl IoEvent {
    /// Creates a new `IoEvent`.
    pub fn new() -> io::Result<IoEvent> {
        let (writer, reader) = socket_pair()?;
        writer.set_send_buffer_size(1)?;
        reader.set_recv_buffer_size(1)?;

        Ok(IoEvent(Arc::new(Inner {
            flag: AtomicBool::new(false),
            writer,
            reader: Async::new(reader)?,
        })))
    }

    /// Sets the flag to `true`.
    pub fn notify(&self) {
        // Publish all in-memory changes before setting the flag.
        atomic::fence(Ordering::SeqCst);

        // If the flag is not set...
        if !self.0.flag.load(Ordering::SeqCst) {
            // If this thread sets it...
            if !self.0.flag.swap(true, Ordering::SeqCst) {
                // Trigger an I/O event by writing a byte into the sending socket.
                let _ = (&self.0.writer).write(&[1]);
                let _ = (&self.0.writer).flush();
            }
        }
    }

    /// Sets the flag to `false`.
    pub fn clear(&self) -> bool {
        // Read all available bytes from the receiving socket.
        while self.0.reader.get_ref().read(&mut [0; 64]).is_ok() {}
        let value = self.0.flag.swap(false, Ordering::SeqCst);

        // Publish all in-memory changes after clearing the flag.
        atomic::fence(Ordering::SeqCst);
        value
    }

    /// Waits until notified.
    ///
    /// You should assume notifications may spuriously occur.
    pub async fn notified(&self) {
        self.0
            .reader
            .with(|_| match self.0.flag.load(Ordering::SeqCst) {
                true => Ok(()),
                false => Err(io::Error::new(io::ErrorKind::WouldBlock, "")),
            })
            .await
            .expect("failure while waiting on a self-pipe");
    }
}

/// TODO
#[cfg(unix)]
fn socket_pair() -> io::Result<(Socket, Socket)> {
    let (sock1, sock2) = Socket::pair(Domain::unix(), Type::stream(), None)?;
    sock1.set_nonblocking(true)?;
    sock2.set_nonblocking(true)?;
    Ok((sock1, sock2))
}

/// TODO
/// TODO The only portable way of manually triggering I/O events is to create a socket and
/// send/receive dummy data on it. This pattern is also known as "the self-pipe trick".
/// See the links below for more information.
///
/// https://github.com/python-trio/trio/blob/master/trio/_core/_wakeup_socketpair.py
/// https://stackoverflow.com/questions/24933411/how-to-emulate-socket-socketpair-on-windows
/// https://gist.github.com/geertj/4325783
#[cfg(windows)]
fn socket_pair() -> io::Result<(Socket, Socket)> {
    // Create a temporary listener.
    let listener = Socket::new(Domain::ipv4(), Type::stream(), None)?;
    listener.bind(&SocketAddr::from(([127, 0, 0, 1], 0)).into())?;
    listener.listen(1)?;

    // First socket: start connecting to the listener.
    let sock1 = Socket::new(Domain::ipv4(), Type::stream(), None)?;
    sock1.set_nonblocking(true)?;
    let _ = sock1.set_nodelay(true)?;
    let _ = sock1.connect(&listener.local_addr()?);

    // Second socket: accept a connection from the listener.
    let (sock2, _) = listener.accept()?;
    sock2.set_nonblocking(true)?;
    let _ = sock2.set_nodelay(true)?;

    Ok((sock1, sock2))
}
