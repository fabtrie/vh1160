//! Control library for the Vector VH1160 / VH1150 ECU test box.
//!
//! The box powers an ECU through relays on terminals 30/15/31, sets its supply
//! voltage, measures voltages and current, and injects CAN/LIN line faults. It
//! shows up as an FTDI virtual COM port (230400 8N1) and speaks a small ASCII
//! protocol: one command letter, optional digits, then `\r`. Only a few
//! commands answer, with a fixed-length reply that repeats the command letter
//! and ends in `\r`.
//!
//! Vector's `VHDevDLL.dll` only works inside CANoe, so the protocol here was
//! read out of that DLL (CANoe 17, v2.9.0.0). The driver mirrors what the DLL
//! sends, including its waits and its hardware-revision checks.

pub mod device;
pub mod error;
pub mod protocol;
pub mod sim;
pub mod transport;

pub use device::{find_devices, list_ports, PortEntry, Vh1160};
pub use error::{Error, Result};
pub use protocol::{
    DeviceState, LinCabSupply, LinPeriodicFault, LineInterrupt, LineShort, Measurement, Terminal,
    Version,
};
pub use sim::{SimHandle, SimTransport};
pub use transport::{SerialTransport, Transport};
