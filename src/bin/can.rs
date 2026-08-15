#![no_std]
#![no_main]

use embassy_time::{Duration, Timer};
use embedded_can::{Frame, Id};
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output};
use esp_hal::spi::master::{Config, Spi};
use esp_hal::spi::Mode;
use esp_hal::time::Rate;
use esp_println::logger::init_logger_from_env;
use esp_println::println;
use mcp2515::MCP2515;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("PANIC: {}", info);
    loop {}
}

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) {
    esp_alloc::heap_allocator!(size: 72 * 1024);
    init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max());
    let peripherals = esp_hal::init(config);

    let sw_int =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    println!();
    println!("TEST MCP2515");
    println!();

    let mut delay = Delay::new();

    let cs = Output::new(
        peripherals.GPIO5,
        Level::High,
        esp_hal::gpio::OutputConfig::default(),
    );

    let spi = Spi::new(
        peripherals.SPI2,
        Config::default()
            .with_frequency(Rate::from_mhz(1))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(peripherals.GPIO18)
    .with_mosi(peripherals.GPIO23)
    .with_miso(peripherals.GPIO19);

    let spi_dev = ExclusiveDevice::new(spi, cs, delay).unwrap();

    let mut can = MCP2515::new(spi_dev);

    println!("SPI configurado a 1 MHz.");
    println!("Tentando inicializar o MCP2515...");
    println!();

    let result = can.init(
        &mut delay,
        mcp2515::Settings {
            mode: mcp2515::regs::OpMode::ListenOnly,
            can_speed: mcp2515::CanSpeed::Kbps500,
            mcp_speed: mcp2515::McpSpeed::MHz8,
            clkout_en: false,
        },
    );

    if let Err(e) = result {
        println!("Erro ao inicializar o MCP2515: {:?}", e);
        println!("{:?}", e);
        loop {
            Timer::after(Duration::from_secs(5)).await;
        }
    }

    println!("MCP2515 inicializado com sucesso!");
    println!();
    println!("Aguardando mensagens...");

    let mut frame_count = 0;
    loop {
        match can.read_message() {
            Ok(frame) => {
                frame_count += 1;
                println!("Frame {}: {:?}", frame_count, frame);
                print_frame(frame_count, &frame);
            }
            Err(e) => {
                println!("Erro ao ler mensagem: {:?}", e);
            }
        }
        Timer::after(Duration::from_millis(1)).await;
    }
}

fn print_frame<F>(count: u64, frame: &F)
where
    F: Frame,
{
    match frame.id() {
        Id::Standard(id) => {
            println!(
                "#{:08} ID=0x{:03X} DLC={} DATA={:02X?}",
                count,
                id.as_raw(),
                frame.dlc(),
                frame.data()
            );
        }
        Id::Extended(id) => {
            println!(
                "#{:08} ID=0x{:08X} DLC={} DATA={:02X?}",
                count,
                id.as_raw(),
                frame.dlc(),
                frame.data()
            );
        }
    }
}
