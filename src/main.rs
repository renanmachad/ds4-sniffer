#![no_std]
#![no_main]

mod can;

use core::fmt::Display;
use core::mem::size_of;
use core::slice::from_raw_parts;
use embassy_executor::Spawner;
use embassy_time::Duration;
use esp_hal::clock::CpuClock;
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;
use esp_radio::esp_now::BROADCAST_ADDRESS;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[repr(C)]
struct CarPayload {
    turbo_press: f32,
    water_temp: f32,
}

impl Display for CarPayload {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "turbo_press: {}, water_temp: {}",
            self.turbo_press, self.water_temp
        )
    }
}
impl CarPayload {
    fn new(turbo_press: f32, water_temp: f32) -> Self {
        Self {
            turbo_press,
            water_temp,
        }
    }
}

esp_bootloader_esp_idf::esp_app_desc!();
#[esp_rtos::main]
async fn main(_spw: Spawner) {
    esp_alloc::heap_allocator!(size: 72 * 1024);
    esp_println::logger::init_logger_from_env();
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let rng = esp_hal::rng::Rng::new();

    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    let wifi = peripherals.WIFI;
    let controller = esp_radio::wifi::WifiController::new(wifi, Default::default()).unwrap();

    let mut esp_now = controller.esp_now();
    esp_now.set_channel(11).unwrap();
    println!("esp-now version {}", esp_now.version().unwrap());

    loop {
        // No topo do loop
        embassy_time::Timer::after(Duration::from_millis(50)).await;
        let random_turbo_press = ((rng.random() % 150) as f32) / 100.0;

        let random_water_temp = 70.0 + ((rng.random() % 40) as f32);
        let payload = CarPayload::new(random_turbo_press, random_water_temp);
        let bytes: &[u8] = unsafe {
            from_raw_parts(
                &payload as *const CarPayload as *const u8,
                size_of::<CarPayload>(),
            )
        };
        println!("Sending data {}", payload);
        let status = esp_now.send_async(&BROADCAST_ADDRESS, bytes).await;
        println!("done, status: {:?}", status)
    }
}
