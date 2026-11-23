#![no_std]
#![no_main]

use embassy_time::{Duration, Instant, Timer};
use embedded_can::{Frame, Id};
use embedded_hal::delay::DelayNs;
use embedded_hal::spi::SpiDevice;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output};
use esp_hal::spi::master::{Config, Spi};
use esp_hal::spi::Mode;
use esp_hal::time::Rate;
use esp_println::logger::init_logger_from_env;
use esp_println::println;
use mcp2515::regs::{CanStat, OpMode, Rec, Tec};
use mcp2515::MCP2515;

/// Seguranca: por padrao NUNCA sai do listen-only (nao ACKa nem gera error frames no carro).
/// Mude para `true` so depois de validar o bitrate/cristal em listen-only.
const ENABLE_NORMAL_MODE: bool = false;
/// Tempo em listen-only (com frames validos) antes de tentar o modo normal.
const LISTEN_SECS: u64 = 10;
/// Se TEC OU REC passar disso em modo normal, volta para listen-only (error warning = 96).
const ERR_ABORT: u8 = 96;

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

    let mut spi_dev = ExclusiveDevice::new(spi, cs, delay).unwrap();

    // Teste de SPI bruto (antes da crate): RESET e leitura de CANSTAT/CANCTRL.
    // Apos o reset o MCP2515 responde CANSTAT=0x80 e CANCTRL=0x87.
    let mut rst = [0xC0u8];
    let _ = spi_dev.transfer_in_place(&mut rst);
    delay.delay_ms(10);
    let mut canstat = [0x03u8, 0x0E, 0x00];
    let _ = spi_dev.transfer_in_place(&mut canstat);
    let mut canctrl = [0x03u8, 0x0F, 0x00];
    let _ = spi_dev.transfer_in_place(&mut canctrl);
    println!(
        "SPI bruto: CANSTAT={:02X} (esperado 80) CANCTRL={:02X} (esperado 87)",
        canstat[2], canctrl[2]
    );
    if canstat[2] != 0x80 || canctrl[2] != 0x87 {
        println!("MCP2515 NAO responde na SPI. Veja fios SCK/SI/SO/CS, VCC, GND e o divisor do SO.");
        println!("FF = MISO preso em 1 (sem fio/CS); 00 = MISO preso em 0 (sem resposta).");
        loop {
            Timer::after(Duration::from_secs(5)).await;
        }
    }

    let mut can = MCP2515::new(spi_dev);

    println!("SPI configurado a 1 MHz.");
    println!("Tentando inicializar o MCP2515...");
    println!();

    let result = can.init(
        &mut delay,
        mcp2515::Settings {
            // Sempre sobe em listen-only; a troca para Normal e feita depois, com checagens.
            mode: OpMode::ListenOnly,
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
    log_status(&mut can, "apos init");
    println!();
    println!("Aguardando mensagens...");

    let start = Instant::now();
    let mut last_status = Instant::now();
    let mut frame_count: u64 = 0;
    let mut read_errs: u64 = 0;
    let mut normal = false;
    loop {
        match can.read_message() {
            Ok(frame) => {
                frame_count += 1;
                print_frame(frame_count, &frame);
            }
            // Sem mensagem pendente tambem cai aqui; so contamos para nao inundar a serial.
            Err(_) => read_errs += 1,
        }

        if last_status.elapsed() >= Duration::from_secs(1) {
            last_status = Instant::now();
            println!("[status] frames={} read_errs={}", frame_count, read_errs);
            let (tec, rec) = log_status(&mut can, if normal { "NORMAL" } else { "LISTEN" })
                .unwrap_or((0, 0));

            // Protecao: erros subindo (TX ou RX) em modo normal -> volta para listen-only.
            if normal && (tec > ERR_ABORT || rec > ERR_ABORT) {
                println!("TEC/REC alto em modo normal -> voltando para ListenOnly");
                let _ = can.set_mode(OpMode::ListenOnly);
                normal = false;
            }
        }

        // Troca para modo normal (sem enviar nada) so se ja recebemos frames validos.
        if ENABLE_NORMAL_MODE
            && !normal
            && frame_count > 0
            && start.elapsed() >= Duration::from_secs(LISTEN_SECS)
        {
            println!("Trocando para modo NORMAL (somente ACK, sem transmitir)...");
            match can.set_mode(OpMode::Normal) {
                Ok(()) => normal = true,
                Err(e) => println!("Falha ao entrar em modo normal: {:?}", e),
            }
            log_status(&mut can, "apos set_mode(Normal)");
        }

        Timer::after(Duration::from_millis(1)).await;
    }
}

/// Le CANSTAT (modo real do chip), TEC e REC (contadores de erro de TX/RX) e imprime.
/// Retorna (TEC, REC). TEC/REC subindo em modo normal => bitrate/cristal errado ou problema eletrico.
fn log_status<SPI: SpiDevice>(can: &mut MCP2515<SPI>, tag: &str) -> Option<(u8, u8)> {
    let stat: Result<CanStat, _> = can.read_register();
    let tec: Result<Tec, _> = can.read_register();
    let rec: Result<Rec, _> = can.read_register();
    match (stat, tec, rec) {
        (Ok(stat), Ok(tec), Ok(rec)) => {
            let tec = tec.into_bytes()[0];
            let rec = rec.into_bytes()[0];
            println!("[{}] opmod={:?} TEC={} REC={}", tag, stat.opmod(), tec, rec);
            Some((tec, rec))
        }
        _ => {
            println!("[{}] falha ao ler CANSTAT/TEC/REC (SPI?)", tag);
            None
        }
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
