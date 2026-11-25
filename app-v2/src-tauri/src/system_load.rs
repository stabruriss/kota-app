//! App-wide memory readings only. Thresholds and lamp history live in the page.

use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, State};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Sample {
    at_ms: u64,
    interval_ms: Option<u64>,
    total_ram_bytes: u64,
    compressed_bytes: u64,
    kota_page_bytes: Option<u64>,
    swap_bytes_per_sec: Option<f64>,
}

#[derive(Default)]
pub(crate) struct SystemLoad {
    latest: Mutex<Option<Sample>>,
}

#[tauri::command]
pub(crate) fn system_load_snapshot(state: State<'_, SystemLoad>) -> Option<Sample> {
    state.latest.lock().unwrap().clone()
}

pub(crate) fn start(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let app = app.clone();
        if let Err(err) = std::thread::Builder::new()
            .name("kota-system-load".into())
            .spawn(move || macos::run(app))
        {
            eprintln!("Kota system load sampler could not start: {err}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app; // Unsupported platforms leave the snapshot null.
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy)]
struct Counters {
    at_ms: u64,
    swapins: u64,
    swapouts: u64,
}

#[cfg(any(target_os = "macos", test))]
fn interval_and_swap(
    previous: Option<Counters>,
    current: Counters,
    page_size: u64,
) -> (Option<u64>, Option<f64>) {
    let Some(previous) = previous else {
        return (None, None);
    };
    // Wall time includes sleep on macOS. Discard clock regressions and long
    // gaps rather than treating a wake-up burst as a normal 30-second sample.
    let Some(ms) = current
        .at_ms
        .checked_sub(previous.at_ms)
        .filter(|ms| *ms > 0 && *ms <= 75_000)
    else {
        return (None, None);
    };
    let pages = current
        .swapins
        .checked_sub(previous.swapins)
        .zip(current.swapouts.checked_sub(previous.swapouts))
        .and_then(|(ins, outs)| ins.checked_add(outs));
    let rate = pages.map(|pages| pages as f64 * page_size as f64 * 1000.0 / ms as f64);
    (Some(ms), rate)
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{interval_and_swap, Counters, Sample, SystemLoad};
    use objc2::{msg_send, rc::Retained, runtime::AnyObject, sel};
    use std::cell::OnceCell;
    use std::mem::{size_of, MaybeUninit};
    use std::sync::{mpsc, Once};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use tauri::{AppHandle, Emitter, Manager};

    const PERIOD: Duration = Duration::from_secs(30);
    static MISSING_PID_SELECTOR: Once = Once::new();
    thread_local! {
        // Accessed only from main-thread callbacks. Kota's main window lives
        // until process exit and is never rebuilt; page process IDs may change.
        static PAGE_VIEW: OnceCell<Retained<AnyObject>> = const { OnceCell::new() };
    }

    pub(super) fn run(app: AppHandle) {
        let Some(total_ram_bytes) = total_ram_bytes() else {
            eprintln!("Kota system load sampler could not read physical memory size");
            return;
        };
        // SAFETY: sysconf has no pointer arguments; a nonpositive result is an error.
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if page_size <= 0 {
            eprintln!("Kota system load sampler could not read page size");
            return;
        }
        let page_size = page_size as u64;
        // Acquire one host send right for this process-lifetime worker, not a
        // fresh right on every sample (mach_host_self increments its refcount).
        // This system-provided symbol uses libc's binding to avoid a new dependency.
        #[allow(deprecated)]
        let host = unsafe { libc::mach_host_self() };
        let mut previous = None;
        loop {
            let started = Instant::now();
            let kota_page_bytes = page_pid(&app).and_then(footprint);
            if let Some(vm) = vm_statistics(host) {
                let at_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let current = Counters {
                    at_ms,
                    swapins: vm.swapins,
                    swapouts: vm.swapouts,
                };
                let (interval_ms, swap_bytes_per_sec) =
                    interval_and_swap(previous, current, page_size);
                previous = Some(current);
                let sample = Sample {
                    at_ms,
                    interval_ms,
                    total_ram_bytes,
                    compressed_bytes: u64::from(vm.compressor_page_count) * page_size,
                    kota_page_bytes,
                    swap_bytes_per_sec,
                };
                *app.state::<SystemLoad>().latest.lock().unwrap() = Some(sample.clone());
                let _ = app.emit("system-load://sample", sample);
            } else {
                // Never publish a failed kernel read as zero memory or bridge a
                // missing sample with a seemingly valid rate.
                previous = None;
            }
            // No catch-up queue after sleep or a delayed main-thread callback.
            std::thread::sleep(PERIOD.saturating_sub(started.elapsed()));
        }
    }

    fn page_pid(app: &AppHandle) -> Option<libc::pid_t> {
        let (send, receive) = mpsc::channel();
        let main_app = app.clone();
        // Only the worker waits; no WKWebView pointer or Objective-C object
        // crosses threads. A late callback cannot change this sample's null.
        app.run_on_main_thread(move || {
            PAGE_VIEW.with(|stored| {
                if let Some(view) = stored.get() {
                    let _ = send.send(read_page_pid(view));
                    return;
                }
                let Some(window) = main_app.get_webview_window("main") else {
                    return; // No view yet: retry at the next sample.
                };
                let _ = window.with_webview(move |webview| {
                    // SAFETY: Tauri provides a live WKWebView on the main thread.
                    // Retain our own reference, never take ownership of Tauri's.
                    // Call with_webview only until we have the view: the locked
                    // tauri-runtime-wry 2.11.2 leaks three retains per invocation.
                    if let Some(view) =
                        unsafe { Retained::<AnyObject>::retain(webview.inner().cast()) }
                    {
                        PAGE_VIEW.with(|stored| {
                            let _ = stored.set(view);
                        });
                    }
                    PAGE_VIEW.with(|stored| {
                        let _ = send.send(stored.get().and_then(|view| read_page_pid(view)));
                    });
                });
            });
        })
        .ok()?;
        receive.recv_timeout(Duration::from_secs(1)).ok().flatten()
    }

    fn read_page_pid(view: &AnyObject) -> Option<libc::pid_t> {
        // SAFETY: only called on the main thread with our retained WKWebView;
        // this private selector returns pid_t and is checked before dispatch.
        unsafe {
            let available: bool = msg_send![view, respondsToSelector: sel!(_webProcessIdentifier)];
            if !available {
                MISSING_PID_SELECTOR.call_once(|| {
                    eprintln!("Kota system load: WKWebView page PID is unavailable");
                });
                return None;
            }
            let pid: libc::pid_t = msg_send![view, _webProcessIdentifier];
            (pid > 0).then_some(pid)
        }
    }

    fn total_ram_bytes() -> Option<u64> {
        let mut bytes = 0_u64;
        let mut len = size_of::<u64>();
        // SAFETY: a u64-sized output buffer matches hw.memsize; no input buffer.
        let result = unsafe {
            libc::sysctlbyname(
                c"hw.memsize".as_ptr(),
                (&mut bytes as *mut u64).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (result == 0 && len == size_of::<u64>() && bytes > 0).then_some(bytes)
    }

    fn vm_statistics(host: libc::host_t) -> Option<libc::vm_statistics64> {
        let mut info = MaybeUninit::<libc::vm_statistics64>::zeroed();
        let mut count = libc::HOST_VM_INFO64_COUNT;
        // SAFETY: the buffer and count match HOST_VM_INFO64, including the
        // compressor and swap counters. Read it only after a complete success.
        let result = unsafe {
            libc::host_statistics64(
                host,
                libc::HOST_VM_INFO64,
                info.as_mut_ptr().cast(),
                &mut count,
            )
        };
        if result != libc::KERN_SUCCESS || count != libc::HOST_VM_INFO64_COUNT {
            return None;
        }
        Some(unsafe { info.assume_init() })
    }

    pub(super) fn footprint(pid: libc::pid_t) -> Option<u64> {
        let mut info = MaybeUninit::<libc::rusage_info_v4>::zeroed();
        // SAFETY: proc_pid_rusage writes the flavor's struct into the buffer,
        // despite libc's opaque rusage_info_t pointer-to-pointer signature.
        let result =
            unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, info.as_mut_ptr().cast()) };
        (result == 0).then(|| unsafe { info.assume_init() }.ri_phys_footprint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counters(at_ms: u64, swapins: u64, swapouts: u64) -> Counters {
        Counters {
            at_ms,
            swapins,
            swapouts,
        }
    }

    #[test]
    fn first_sample_has_no_interval_or_rate() {
        assert_eq!(
            interval_and_swap(None, counters(30_000, 100, 200), 16_384),
            (None, None)
        );
    }

    #[test]
    fn rate_uses_both_swap_counters_and_actual_interval() {
        let previous = Some(counters(1_000, 100, 200));
        assert_eq!(
            interval_and_swap(previous, counters(31_000, 130, 230), 16_384),
            (Some(30_000), Some(32_768.0)),
        );
        assert_eq!(
            interval_and_swap(previous, counters(61_000, 130, 230), 16_384),
            (Some(60_000), Some(16_384.0)),
        );
        assert_eq!(
            interval_and_swap(previous, counters(31_000, 100, 200), 16_384),
            (Some(30_000), Some(0.0)),
        );
    }

    #[test]
    fn gaps_over_75_seconds_and_nonpositive_intervals_are_unknown() {
        let previous = Some(counters(1_000, 10, 20));
        assert_eq!(
            interval_and_swap(previous, counters(76_000, 10, 20), 4096),
            (Some(75_000), Some(0.0)),
        );
        for at_ms in [76_001, 300_000, 1_000, 999] {
            assert_eq!(
                interval_and_swap(previous, counters(at_ms, 30, 40), 4096),
                (None, None)
            );
        }
        // The long-gap sample becomes the baseline; no sleep traffic leaks
        // into the next normal interval.
        assert_eq!(
            interval_and_swap(
                Some(counters(300_000, 30, 40)),
                counters(330_000, 30, 40),
                4096
            ),
            (Some(30_000), Some(0.0)),
        );
    }

    #[test]
    fn decreasing_or_wrapped_counters_do_not_produce_a_rate() {
        let previous = Some(counters(1_000, 100, 200));
        for current in [counters(31_000, 99, 999), counters(31_000, 999, 199)] {
            assert_eq!(
                interval_and_swap(previous, current, 4096),
                (Some(30_000), None)
            );
        }
        assert_eq!(
            interval_and_swap(
                Some(counters(1_000, u64::MAX, 200)),
                counters(31_000, 0, 201),
                4096
            ),
            (Some(30_000), None),
        );
        assert_eq!(
            interval_and_swap(
                Some(counters(1_000, 0, 0)),
                counters(31_000, u64::MAX, 1),
                4096
            ),
            (Some(30_000), None),
        );
    }

    #[test]
    fn payload_uses_the_contract_field_names_and_explicit_nulls() {
        let sample = Sample {
            at_ms: 123,
            interval_ms: None,
            total_ram_bytes: 16 * (1 << 30),
            compressed_bytes: 2 * (1 << 30),
            kota_page_bytes: None,
            swap_bytes_per_sec: None,
        };
        assert_eq!(
            serde_json::to_value(&sample).unwrap(),
            serde_json::json!({
                "atMs": 123,
                "intervalMs": null,
                "totalRamBytes": 17179869184_u64,
                "compressedBytes": 2147483648_u64,
                "kotaPageBytes": null,
                "swapBytesPerSec": null,
            }),
        );
        let value = serde_json::to_value(Sample {
            interval_ms: Some(30_000),
            kota_page_bytes: Some(1 << 30),
            swap_bytes_per_sec: Some(12.5),
            ..sample
        })
        .unwrap();
        assert_eq!(value["intervalMs"], 30_000);
        assert_eq!(value["kotaPageBytes"], 1 << 30);
        assert_eq!(value["swapBytesPerSec"], 12.5);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn current_process_has_a_real_footprint() {
        let pid = unsafe { libc::getpid() };
        assert!(macos::footprint(pid).expect("read this process's rusage_info_v4") > 0);
    }
}
