use std::io::{Read, Write};
use std::time::{Duration, Instant};

use crate::error::Result;

/// A byte channel to the box. The protocol (tags, retries, waits) lives in
/// [`crate::Vh1160`]; a transport only moves bytes.
pub trait Transport: Send {
    /// Drops whatever has arrived but was not read yet.
    fn clear_input(&mut self) -> Result<()>;
    fn write_all(&mut self, data: &[u8]) -> Result<()>;
    /// Reads up to `n` bytes, waiting at most the transport's timeout.
    fn read(&mut self, n: usize) -> Result<Vec<u8>>;
    /// Waits between commands. The simulator can skip real waiting.
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

pub struct SerialTransport {
    port: Box<dyn serialport::SerialPort>,
    timeout: Duration,
}

impl SerialTransport {
    pub const BAUD: u32 = 230_400;
    const DEFAULT_TIMEOUT: Duration = Duration::from_millis(500);

    pub fn open(path: &str) -> Result<Self> {
        Self::open_with_timeout(path, Self::DEFAULT_TIMEOUT)
    }

    pub fn open_with_timeout(path: &str, timeout: Duration) -> Result<Self> {
        let port = serialport::new(path, Self::BAUD)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(20))
            .open()?;
        Ok(Self { port, timeout })
    }
}

impl Transport for SerialTransport {
    fn clear_input(&mut self) -> Result<()> {
        self.port.clear(serialport::ClearBuffer::Input)?;
        Ok(())
    }

    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        self.port.write_all(data)?;
        self.port.flush()?;
        Ok(())
    }

    fn read(&mut self, n: usize) -> Result<Vec<u8>> {
        let deadline = Instant::now() + self.timeout;
        let mut buf = vec![0u8; n];
        let mut got = 0;
        while got < n && Instant::now() < deadline {
            match self.port.read(&mut buf[got..]) {
                Ok(k) => got += k,
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => return Err(e.into()),
            }
        }
        buf.truncate(got);
        Ok(buf)
    }
}
