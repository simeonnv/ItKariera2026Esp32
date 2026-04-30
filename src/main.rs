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

// static TIMERS: StaticCell<[OneShotTimer<Blocking>; 1]> = StaticCell::new();
// static VIBRATION: StaticCell<Output> = StaticCell::new();
// static RTC: StaticCell<Rtc> = StaticCell::new();
esp_bootloader_esp_idf::esp_app_desc!();

#[main]
async fn main(_low_prio_spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::_80MHz));
    esp_alloc::heap_allocator!(size: 73728);

    let mut delay = Delay::new();
    let address = 0x1E;

    const REG_CTRL_REG1: u8 = 0x20;
    const REG_CTRL_REG3: u8 = 0x22;
    const REG_CTRL_REG5: u8 = 0x24;
    const REG_OUT_X_L: u8 = 0x28;

    const SENSITIVITY_4G: f32 = 6842.0;

    // 0011110b
    let i2c_config = I2cConfig::default()
        .with_frequency(Rate::from_khz(10))
        .with_timeout(BusTimeout::Maximum);
    let mut i2c = I2c::new(peripherals.I2C0, i2c_config)
        .unwrap()
        .with_sda(peripherals.GPIO21)
        .with_scl(peripherals.GPIO22);

    i2c.write(address, &[REG_CTRL_REG1, 0x70]).unwrap();
    i2c.write(address, &[REG_CTRL_REG3, 0x00]).unwrap();
    i2c.write(address, &[REG_CTRL_REG5, 0x40]).unwrap();
    delay.delay_ms(20_u32);
    let mut data = [0_u8; 6];

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

    display_driver.flush().ok();
    info!("display flush");

    // esp_hal_embassy::init(embassy_timers);

    defmt::info!("setting global time forever");

    let center = Point::new(120, 120);
    let arrow_length = 80.0f32;

    loop {
        match i2c.write_read(address, &[REG_OUT_X_L | 0x80], &mut data) {
            Ok(_) => {
                use micromath::F32Ext;

                let raw_x = i16::from_le_bytes([data[0], data[1]]) as f32;
                let raw_y = i16::from_le_bytes([data[2], data[3]]) as f32;
                let raw_z = i16::from_le_bytes([data[4], data[5]]) as f32;

                // 1. Scale raw data to Gauss

                let x = raw_x / SENSITIVITY_4G;
                let y = raw_y / SENSITIVITY_4G;
                let z = raw_z / SENSITIVITY_4G;

                info!("x: {}, y: {}m z: {}", x, y, z);

                let x_offset = -0.1897;
                let y_offset = -0.5750;

                let cal_x = x - x_offset;
                let cal_y = y - y_offset;

                let mut yaw_rad = cal_y.atan2(cal_x) + core::f32::consts::PI;
                // let mut yaw = yaw_rad * (180.0 / core::f32::consts::PI);

                // if yaw < 0.0 {
                //     yaw += 360.0;
                // }

                if yaw_rad < 0.0 {
                    yaw_rad += core::f32::consts::PI * 2.;
                }

                // info!("Yaw: grad: {} rad: {}°", yaw, yaw_rad);

                let end_x = (center.x as f32) + arrow_length * yaw_rad.cos();
                let end_y = (center.y as f32) + arrow_length * yaw_rad.sin();
                let end_point = Point::new(end_x as i32, end_y as i32);

                display_driver.clear();

                Circle::new(center - Point::new(3, 3), 6)
                    .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
                    .draw(&mut display_driver)
                    .unwrap();

                Line::new(center, end_point)
                    .into_styled(PrimitiveStyle::with_stroke(Rgb565::CYAN, 3))
                    .draw(&mut display_driver)
                    .unwrap();

                let text_style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLUE);

                Text::new("N", Point::new(120, 10), text_style)
                    .draw(&mut display_driver)
                    .unwrap();

                Text::new("E", Point::new(230, 120), text_style)
                    .draw(&mut display_driver)
                    .unwrap();

                Text::new("W", Point::new(0, 120), text_style)
                    .draw(&mut display_driver)
                    .unwrap();

                Text::new("S", Point::new(120, 230), text_style)
                    .draw(&mut display_driver)
                    .unwrap();

                display_driver.flush().ok();
            }

            Err(e) => info!("Read Error: {:?}", e),
        }
    }

    // loop {}
}
