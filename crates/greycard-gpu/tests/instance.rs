//! Instances made on several threads at once, as a test binary makes
//! them, one a test.
//!
//! On NVIDIA's Linux driver two `wgpu::Instance::new` racing each other
//! end the process with a jump to a null pointer inside the driver's
//! loader negotiation; two threads doing it back to back for two
//! seconds died every time we ran it. Through [`greycard_gpu::instance`]
//! they take turns. Elsewhere the test is only cheap.

use std::time::{Duration, Instant};

use greycard_gpu::wgpu;

#[test]
fn instances_made_on_two_threads_at_once_do_not_crash() {
    let end = Instant::now() + Duration::from_secs(2);
    let threads: Vec<_> = (0..2)
        .map(|_| {
            std::thread::spawn(move || {
                let mut made = 0u32;
                while Instant::now() < end {
                    let instance = greycard_gpu::instance();
                    let _ = pollster::block_on(instance.request_adapter(
                        &wgpu::RequestAdapterOptions {
                            power_preference: wgpu::PowerPreference::HighPerformance,
                            ..Default::default()
                        },
                    ));
                    made += 1;
                }
                made
            })
        })
        .collect();
    let made: u32 = threads.into_iter().map(|t| t.join().unwrap()).sum();
    assert!(made >= 2, "made {made} instances");
}

/// The probe the window's choice of adapter rests on finds the
/// adapter a request on the usual instance is given: a machine with
/// one to draw on is not taken for a machine with none.
#[test]
fn the_probe_finds_the_adapter_a_request_gets() {
    let instance = greycard_gpu::instance();
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
        assert!(
            std::env::var_os("GREYCARD_REQUIRE_GPU").is_none_or(|v| v.is_empty()),
            "no adapter to probe for and GREYCARD_REQUIRE_GPU is set"
        );
        println!("SKIPPED: no adapter to probe for");
        return;
    };
    let got = adapter.get_info();
    let found = greycard_gpu::adapters(got.backend.into());
    assert!(
        found.iter().any(|f| f.name == got.name
            && f.backend == got.backend
            && f.device_type == got.device_type),
        "{got:?} not among {found:?}"
    );
}
