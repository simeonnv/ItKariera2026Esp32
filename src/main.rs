#![deny(clippy::large_futures)]
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![deny(clippy::unwrap_used)]
#![feature(impl_trait_in_assoc_type)]

extern crate alloc;

use alloc::boxed::Box;
use defmt::info;
use embassy_embedded_hal::shared_bus::blocking::spi::SpiDevice;
use embedded_graphics::Drawable;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::{Point, Primitive, RgbColor};
use embedded_graphics::primitives::{Circle, Line, PrimitiveStyle};
use embedded_graphics::text::Text;
use embedded_hal::delay::DelayNs;
use embedded_hal::spi;
use esp_backtrace as _;
use esp_hal::i2c::master::{BusTimeout, Config as I2cConfig, I2c};
use esp_hal::spi::Mode;
use esp_hal::spi::master::Config as SpiConfig;
use esp_hal::spi::master::Spi;
use esp_hal::time::Rate;
use esp_println as _;

use async_debounce::Debouncer;
use bma423::{
    Bma423, Error, FeatureInterruptStatus, InterruptDirection, PowerControlFlag, Uninitialized,
};
use core::f32::consts::PI;
use core::future;
use embassy_executor::Spawner;
use embassy_futures::select::{Either, Either4};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{Duration, Timer};
use embedded_hal::i2c::{self, ErrorType};
use embedded_hal_async::digital::Wait;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::Blocking;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{AnyPin, Input, InputConfig, Io, Level, Output, OutputConfig, Pull};
use esp_hal::interrupt::Priority;
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rtc_cntl::Rtc;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::timer::{OneShotTimer, PeriodicTimer};
use esp_hal_embassy::{InterruptExecutor, main};
use gc9a01::Gc9a01;
use gc9a01::mode::DisplayConfiguration;
use gc9a01::prelude::{DisplayResolution240x240, DisplayRotation, SPIInterface};
use static_cell::StaticCell;

static TIMERS: StaticCell<[OneShotTimer<Blocking>; 1]> = StaticCell::new();
// static VIBRATION: StaticCell<Output> = StaticCell::new();
// static RTC: StaticCell<Rtc> = StaticCell::new();
esp_bootloader_esp_idf::esp_app_desc!();

/// Run the OS
///
/// We have two task spawners, a low priority one and a high prio one which responds to
/// things like buttons.
#[main]
async fn main(low_prio_spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::_80MHz));
    esp_alloc::heap_allocator!(size: 73728);
    info!("mrazq mnegri");

    // let rtc = RTC.init(Rtc::new(peripherals.LPWR));

    let mut delay = Delay::new();
    let address = 0x1E;

    const REG_CTRL_REG1: u8 = 0x20;
    const REG_CTRL_REG3: u8 = 0x22;
    const REG_OUT_X_L: u8 = 0x28;

    // 0011110b
    let mut buffer = [0_u8; 600];
    let i2c_config = I2cConfig::default()
        .with_frequency(Rate::from_khz(10))
        .with_timeout(BusTimeout::Maximum);
    let mut i2c = I2c::new(peripherals.I2C0, i2c_config)
        .unwrap()
        .with_sda(peripherals.GPIO21)
        .with_scl(peripherals.GPIO22);

    i2c.write(address, &[REG_CTRL_REG1, 0x70]).unwrap();
    i2c.write(address, &[REG_CTRL_REG3, 0x00]).unwrap();
    delay.delay_ms(20_u32);
    let mut data = [0_u8; 6];

    // loop {
    //     match i2c.write_read(address, &[REG_OUT_X_L | 0x80], &mut data) {
    //         Ok(_) => {
    //             use micromath::F32Ext;

    //             let x = i16::from_le_bytes([data[0], data[1]]) as f32;
    //             let y = i16::from_le_bytes([data[2], data[3]]) as f32;
    //             let z = i16::from_le_bytes([data[4], data[5]]) as f32;

    //             let pitch = (-x).atan2((y * y + z * z).sqrt()) * 180.0 / PI;
    //             let roll = y.atan2(z) * 180.0 / PI;

    //             // 2. Calculate Yaw (Heading)
    //             let mut yaw = y.atan2(x) * 180.0 / PI;
    //             if yaw < 0.0 {
    //                 yaw += 360.0;
    //             }

    //             info!("Yaw: {}°, Pitch: {}°, Roll: {}°", yaw, pitch, roll);

    //             // info!("Magnetometer Data: x: {}, y: {}, z: {}", x, y, z);
    //         }
    //         Err(e) => info!("Read Error: {:?}", e),
    //     }
    // }

    // for address in 1..=127 {
    //     match i2c.write(address, &[]) {
    //         Ok(_) => info!("Found device at address: 0x{:02X}", address),
    //         Err(_) => {} // No device at this address, move on
    //     }
    // }

    let spi_bus = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(40))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_miso(peripherals.GPIO19)
    .with_mosi(peripherals.GPIO23)
    .with_sck(peripherals.GPIO18);
    // .with_cs(peripherals.);
    info!("spi bus inited");

    let cs_pin = Output::new(peripherals.GPIO5, Level::Low, OutputConfig::default());
    let spi_device = ExclusiveDevice::new_no_delay(spi_bus, cs_pin).unwrap();
    let dc_pin = Output::new(peripherals.GPIO4, Level::Low, OutputConfig::default());
    let mut reset_pin = Output::new(peripherals.GPIO0, Level::Low, OutputConfig::default());

    let spi_interface = SPIInterface::new(spi_device, dc_pin);
    info!("spi interface inited");

    let mut display_driver = Box::new(Gc9a01::new(
        spi_interface,
        DisplayResolution240x240,
        DisplayRotation::Rotate180,
    ))
    .into_buffered_graphics();
    info!("display driver inited");

    display_driver.reset(&mut reset_pin, &mut delay).unwrap();
    display_driver.init(&mut delay).ok();
    display_driver.clear();
    info!("display setup");

    Line::new(Point::new(0, 0), Point::new(200, 200))
        .into_styled(PrimitiveStyle::with_stroke(Rgb565::GREEN, 10))
        .draw(&mut display_driver)
        .unwrap();

    // let circle_style = PrimitiveStyle::with_fill(Rgb565::RED);
    // Circle::new(Point::new(80, 80), 80)
    //     .into_styled(circle_style)
    //     .draw(&mut display_driver) // Note: draw into the buffered driver
    //     .unwrap();

    // let text_style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLUE);
    // Text::new("Hello Rust!", Point::new(40, 120), text_style)
    //     .draw(&mut display_driver)
    //     .unwrap();

    display_driver.flush().ok();
    info!("display flush");

    // let i2c = i2c::master:: :::new(peripherals.i2c0, i2c_sda, i2c_scl, &i2c::I2cConfig::new())?;
    // i2c::master:c:
    // let wakeup_pins = &mut [(&mut io.pins.gpio7.into_ref(), WakeupLevel::Low)];
    // let rtcio = RtcioWakeupSource::new(wakeup_pins);
    // defmt::info!("sleeping");
    // rtc.sleep_light(&[&rtcio]);
    // defmt::info!("waking up");

    // let embassy_timers = {
    //     let timg0 = TimerGroup::new(peripherals.TIMG0);
    //     let timers = [OneShotTimer::new(timg0.timer0)];
    //     TIMERS.init(timers)
    // };

    // esp_hal_embassy::init(embassy_timers);

    defmt::info!("setting global time forever");

    // Define screen constants
    let center = Point::new(120, 120);
    let arrow_length = 80.0f32;

    loop {
        match i2c.write_read(address, &[REG_OUT_X_L | 0x80], &mut data) {
            Ok(_) => {
                use micromath::F32Ext;

                let x = i16::from_le_bytes([data[0], data[1]]) as f32;
                let y = i16::from_le_bytes([data[2], data[3]]) as f32;
                let z = i16::from_le_bytes([data[4], data[5]]) as f32;
                info!("x: {} y: {} z: {}", x, y, z);

                let yaw_rad = y.atan2(x);
                let end_x = 120.0 + arrow_length * yaw_rad.cos();
                let end_y = 120.0 + arrow_length * yaw_rad.sin();
                let end_point = Point::new(end_x as i32, end_y as i32);

                display_driver.clear(); // Clear buffer

                Circle::new(center - Point::new(3, 3), 6)
                    .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
                    .draw(&mut display_driver)
                    .unwrap();

                Line::new(center, end_point)
                    .into_styled(PrimitiveStyle::with_stroke(Rgb565::CYAN, 3))
                    .draw(&mut display_driver)
                    .unwrap();

                display_driver.flush().ok();
            }
            Err(e) => info!("Read Error: {:?}", e),
        }
    }

    // loop {}
}
