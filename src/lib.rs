#![no_std]

use bit_field::BitField;
use embedded_hal::{
    digital::v2::OutputPin,
    blocking::delay::DelayMs,
};

/// A device driver for the AD9959 direct digital synthesis (DDS) chip.
///
/// This chip provides four independently controllable digital-to-analog output sinusoids with
/// configurable phase, amplitude, and frequency. All channels are inherently synchronized as they
/// are derived off a common system clock.
///
/// The chip contains a configurable PLL and supports system clock frequencies up to 500 MHz.
///
/// The chip supports a number of serial interfaces to improve data throughput, including normal,
/// dual, and quad SPI configurations.
pub struct Ad9959<INTERFACE, DELAY, UPDATE> {
    interface: INTERFACE,
    delay: DELAY,
    reference_clock_frequency: u32,
    system_clock_multiplier: u8,
    io_update: UPDATE,
}

pub trait Interface {
    type Error;

    fn configure_mode(&mut self, mode: Mode) -> Result<(), Self::Error>;

    fn write(&mut self, addr: u8, data: &[u8]) -> Result<(), Self::Error>;

    fn read(&mut self, addr: u8, dest: &mut [u8]) -> Result<(), Self::Error>;
}

#[derive(Copy, Clone)]
pub enum Mode {
    FourBitSerial = 0b11,
}

/// The configuration registers within the AD9959 DDS device. The values of each register are
/// equivalent to the address.
pub enum Register {
    CSR = 0x00,
    FR1 = 0x01,
    FR2 = 0x02,
    CFR = 0x03,
    CFTW0 = 0x04,
    CPOW0 = 0x05,
    ACR = 0x06,
    LSRR = 0x07,
    RDW = 0x08,
    FDW = 0x09,
    CW1 = 0x0a,
    CW2 = 0x0b,
    CW3 = 0x0c,
    CW4 = 0x0d,
    CW5 = 0x0e,
    CW6 = 0x0f,
    CW7 = 0x10,
    CW8 = 0x11,
    CW9 = 0x12,
    CW10 = 0x13,
    CW11 = 0x14,
    CW12 = 0x15,
    CW13 = 0x16,
    CW14 = 0x17,
    CW15 = 0x18,
}

/// Specifies an output channel of the AD9959 DDS chip.
pub enum Channel {
    One = 0,
    Two = 1,
    Three = 2,
    Four = 3,
}

/// Possible errors generated by the AD9959 driver.
#[derive(Debug)]
pub enum Error<InterfaceE> {
    Interface(InterfaceE),
    Bounds,
    Pin,
    Frequency,
    Identification,
}

impl <InterfaceE> From<InterfaceE> for Error<InterfaceE> {
    fn from(interface_error: InterfaceE) -> Self {
        Error::Interface(interface_error)
    }
}

impl <PinE, InterfaceE, INTERFACE, DELAY, UPDATE> Ad9959<INTERFACE, DELAY, UPDATE>
where
    INTERFACE: Interface<Error = InterfaceE>,
    DELAY: DelayMs<u8>,
    UPDATE: OutputPin<Error = PinE>,

{
    pub fn new<RST>(interface: INTERFACE,
                    reset_pin: &mut RST,
                    io_update: UPDATE,
                    delay: DELAY,
                    clock_frequency: u32) -> Result<Self, Error<InterfaceE>>
    where
        RST: OutputPin,
    {
        let mut ad9959 = Ad9959 {
            interface: interface,
            io_update: io_update,
            delay: delay,
            reference_clock_frequency: clock_frequency,
            system_clock_multiplier: 1,
        };

       ad9959.io_update.set_low().or_else(|_| Err(Error::Pin))?;

        // Reset the AD9959
        reset_pin.set_high().or_else(|_| Err(Error::Pin))?;

        // Delay for a clock cycle to allow the device to reset.
        ad9959.delay.delay_ms((1000.0 / clock_frequency as f32) as u8);

        reset_pin.set_low().or_else(|_| Err(Error::Pin))?;

        // multiple gotchas:
        // 1. only four bit is compatible for reads
        //    a) qspi listens (single-bit) on io1 vs dds sends on io0 (two-wire) or io2 (three-wire)
        //    b) two-bit is incompatible because io3=hold=sync_i/o is driven high (might be possible
        //       with io3 not af10 but low gpio)
        // 2. even entering 4 bit mode from 1 bit (reset) requires forcing sync_i/o=io3 low
        //
        // the only simple solution is to use 4-bit mode exlusively and the only way to enter it is
        // to construct the proper padded 4-bit sequence while the dds is still in 1 bit mode
        //
        // data to be sent is is 0x00 0xf6 (write CSR, default all DDS on, MSB first, but four wire)
        // with 4-bit it's then 0x00 0x00 0x00 0x00 0x11 0x11 0x01 0x10
        // and the first byte is taken up as the instruction
        
        // Configure the interface to the desired mode.
       ad9959.interface.configure_mode(Mode::FourBitSerial)?;

       // Program the interface configuration in the AD9959.
       let csr: [u8; 7] = [0x00, 0x00, 0x00, 0x11, 0x11, 0x01, 0x10];
       ad9959.interface.write(0, &csr)?;

       // Latch the configuration registers to make them active.
       ad9959.latch_configuration()?;

       let mut csr: [u8; 1] = [0];
        ad9959.interface.read(Register::CSR as u8, &mut csr)?;
        if csr[0] != 0xf6 {
            return Err(Error::Identification)
        }

       // Set the clock frequency to configure the device as necessary.
       ad9959.set_clock_frequency(clock_frequency)?;
        Ok(ad9959)
    }

    fn latch_configuration(&mut self) -> Result<(), Error<InterfaceE>> {
       self.io_update.set_high().or_else(|_| Err(Error::Pin))?;
       // The SYNC_CLK is 1/4 the system clock frequency. The IO_UPDATE pin must be latched for one
       // full SYNC_CLK pulse to register. For safety, we latch for 5 here.
       self.delay.delay_ms((5000.0 / self.system_clock_frequency()) as u8);
       self.io_update.set_low().or_else(|_| Err(Error::Pin))?;

       Ok(())
    }

    /// Specify the reference clock frequency for the chip.
    ///
    /// Arguments:
    /// * `clock_frequency` - The refrence clock frequency provided to the AD9959 core.
    pub fn set_clock_frequency(&mut self, clock_frequency: u32) -> Result<(), Error<InterfaceE>> {
        // TODO: Check validity of the clock frequency.

        // TODO: If the input clock is above 255 MHz, enable the VCO gain control bit.

        self.reference_clock_frequency = clock_frequency;

        // TODO: Update the system clock frequency given the current PLL configurtation.

        Ok(())
    }

    /// Configure the internal system clock of the chip.
    ///
    /// Arguments:
    /// * frequency` - The desired frequency of the system clock.
    ///
    /// Returns:
    /// The actual frequency configured for the internal system clock.
    pub fn configure_system_clock(&mut self, frequency: f32) -> Result<f32, Error<InterfaceE>> {
        if frequency > 500_000_000.0 {
            return Err(Error::Frequency);
        }

        let prescaler: u8 = match (frequency / self.reference_clock_frequency as f32) as u32 {
            0 => return Err(Error::Frequency),

            // We cannot achieve this frequency with the PLL. Assume the PLL is not used.
            1 | 2 | 3 => 1,
            _ => {
                // Configure the PLL prescaler.
                let mut prescaler = (frequency / self.reference_clock_frequency as f32) as u8;
                if prescaler > 20 {
                    prescaler = 20;
                }

                prescaler
            },
        };

        // TODO: Update / disable any enabled channels?
        let mut fr1: [u8; 3] = [0, 0, 0];
        self.interface.read(Register::FR1 as u8, &mut fr1)?;
        fr1[0].set_bits(2..=6, prescaler);
        let vco_range = frequency > 255e6;
        fr1[0].set_bit(7, vco_range);
        self.interface.write(Register::FR1 as u8, &fr1)?;
        self.system_clock_multiplier = prescaler;

        Ok(self.system_clock_frequency())
    }

    /// Perform a self-test of the communication interface.
    ///
    /// Note:
    /// This modifies the existing channel enables. They are restored upon exit.
    ///
    /// Returns:
    /// True if the self test succeeded. False otherwise.
    pub fn self_test(&mut self) -> Result<bool, Error<InterfaceE>> {
        let mut csr: [u8; 1] = [0];
        self.interface.read(Register::CSR as u8, &mut csr)?;
        let old_csr = csr[0];

        // Enable all channels.
        csr[0].set_bits(4..8, 0xF);
        self.interface.write(Register::CSR as u8, &csr)?;

        // Read back the enable.
        csr[0] = 0;
        self.interface.read(Register::CSR as u8, &mut csr)?;
        if csr[0].get_bits(4..8) != 0xF {
            return Ok(false);
        }

        // Clear all channel enables.
        csr[0].set_bits(4..8, 0x0);
        self.interface.write(Register::CSR as u8, &csr)?;

        // Read back the enable.
        csr[0] = 0xFF;
        self.interface.read(Register::CSR as u8, &mut csr)?;
        if csr[0].get_bits(4..8) != 0 {
            return Ok(false);
        }

        // Restore the CSR.
        csr[0] = old_csr;
        self.interface.write(Register::CSR as u8, &csr)?;

        Ok(true)
    }

    fn system_clock_frequency(&self) -> f32 {
        self.system_clock_multiplier as f32 * self.reference_clock_frequency as f32
    }

    /// Enable an output channel.
    pub fn enable_channel(&mut self, channel: Channel) -> Result<(), Error<InterfaceE>> {
        let mut csr: [u8; 1] = [0];
        self.interface.read(Register::CSR as u8, &mut csr)?;
        csr[0].set_bit(channel as usize + 4, true);
        self.interface.write(Register::CSR as u8, &csr)?;

        Ok(())
    }

    /// Disable an output channel.
    pub fn disable_channel(&mut self, channel: Channel) -> Result<(), Error<InterfaceE>> {
        let mut csr: [u8; 1] = [0];
        self.interface.read(Register::CSR as u8, &mut csr)?;
        csr[0].set_bit(channel as usize + 4, false);
        self.interface.write(Register::CSR as u8, &csr)?;

        Ok(())
    }

    fn modify_channel(&mut self, channel: Channel, register: Register, data: &[u8]) -> Result<(), Error<InterfaceE>> {
        let mut csr: [u8; 1] = [0];
        self.interface.read(Register::CSR as u8, &mut csr)?;

        let mut new_csr = csr;
        new_csr[0].set_bits(4..8, 0);
        new_csr[0].set_bit(4 + channel as usize, true);

        self.interface.write(Register::CSR as u8, &new_csr)?;

        self.interface.write(register as u8, &data)?;

        // Latch the configuration and restore the previous CSR. Note that the re-enable of the
        // channel happens immediately, so the CSR update does not need to be latched.
        self.latch_configuration()?;
        self.interface.write(Register::CSR as u8, &csr)?;

        Ok(())
    }
    
    /// Configure the phase of a specified channel.
    ///
    /// Arguments:
    /// * `channel` - The channel to configure the frequency of.
    /// * `phase_degrees` - The desired phase offset within [0, 360] degrees.
    ///
    /// Returns:
    /// The actual programmed phase offset of the channel in degrees.
    pub fn set_phase(&mut self, channel: Channel, phase_degrees: f32) -> Result<f32, Error<InterfaceE>> {
        if phase_degrees > 360.0 || phase_degrees < 0.0 {
            return Err(Error::Bounds);
        }

        let phase_offset: u16 = (phase_degrees / 360.0 * 2_u32.pow(14) as f32) as u16;
        self.modify_channel(channel, Register::CPOW0, &phase_offset.to_be_bytes())?;
        Ok((phase_offset as f32 / 2_u32.pow(14) as f32) * 360.0)
    }

    /// Configure the amplitude of a specified channel.
    ///
    /// Arguments:
    /// * `channel` - The channel to configure the frequency of.
    /// * `amplitude` - A normalized amplitude setting [0, 1].
    ///
    /// Returns:
    /// The actual normalized amplitude of the channel relative to full-scale range.
    pub fn set_amplitude(&mut self, channel: Channel, amplitude: f32) -> Result<f32, Error<InterfaceE>> {
        if amplitude < 0.0 || amplitude > 1.0 {
            return Err(Error::Bounds);
        }

        let amplitude_control: u16 = (amplitude / 1.0 * 2_u16.pow(10) as f32) as u16;
        let mut acr: [u8; 3] = [0, amplitude_control.to_be_bytes()[0], amplitude_control.to_be_bytes()[1]];

        // Enable the amplitude multiplier for the channel if required.
        acr[1].set_bit(4, amplitude_control < 0x3ff);

        self.modify_channel(channel, Register::ACR, &acr)?;

        Ok(amplitude_control as f32 / 2_u16.pow(10) as f32)
    }

    /// Configure the frequency of a specified channel.
    ///
    /// Arguments:
    /// * `channel` - The channel to configure the frequency of.
    /// * `frequency` - The desired output frequency in Hz.
    ///
    /// Returns:
    /// The actual programmed frequency of the channel.
    pub fn set_frequency(&mut self, channel: Channel, frequency: f32) -> Result<f32, Error<InterfaceE>> {
        if frequency < 0.0 || frequency > self.system_clock_frequency() {
            return Err(Error::Bounds);
        }

        let tuning_word: u32 = ((frequency as f32 / self.system_clock_frequency()) * u32::max_value()
            as f32) as u32;
        self.modify_channel(channel, Register::CFTW0, &tuning_word.to_be_bytes())?;
        Ok((tuning_word as f32 / u32::max_value() as f32) * self.system_clock_frequency())
    }
}
