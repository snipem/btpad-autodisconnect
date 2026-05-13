use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::Parser;
use evdev::Device;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::Connection;

#[derive(Parser)]
#[command(
    name = "btpad-autodisconnect",
    about = "Disconnect Bluetooth gamepad(s) after idle timeout"
)]
struct Args {
    /// Idle timeout in seconds before disconnecting
    #[arg(short, long, default_value = "600")]
    timeout: u64,

    /// Device name substring to match (case-insensitive)
    #[arg(short, long, default_value = "Wireless Controller")]
    name: String,

    /// Print last activity timestamp every second
    #[arg(long)]
    debug: bool,
}

#[zbus::proxy(
    interface = "org.bluez.Device1",
    default_service = "org.bluez"
)]
trait BlueZDevice {
    async fn disconnect(&self) -> zbus::Result<()>;

    #[zbus(property)]
    fn name(&self) -> zbus::Result<String>;
}

#[zbus::proxy(
    interface = "org.freedesktop.DBus.ObjectManager",
    default_service = "org.bluez",
    default_path = "/"
)]
trait ObjectManager {
    async fn get_managed_objects(
        &self,
    ) -> zbus::Result<
        HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>,
    >;
}

// Stable identifier for a device: MAC if available, else physical path.
fn device_id(dev: &Device) -> String {
    if let Some(mac) = dev.unique_name().filter(|s| !s.is_empty()) {
        return mac.to_uppercase();
    }
    dev.physical_path().unwrap_or("unknown").to_owned()
}

fn find_all_input_devices(name_filter: &str) -> Vec<Device> {
    let filter_lower = name_filter.to_lowercase();
    let Ok(dir) = std::fs::read_dir("/dev/input") else { return vec![] };
    dir.flatten()
        .filter_map(|e| Device::open(e.path()).ok())
        .filter(|d| d.name().unwrap_or("").to_lowercase().contains(&filter_lower))
        .collect()
}

fn str_prop<'a>(
    props: &'a HashMap<String, OwnedValue>,
    key: &str,
) -> Option<&'a str> {
    if let zbus::zvariant::Value::Str(s) = &**props.get(key)? {
        Some(s.as_str())
    } else {
        None
    }
}

// Match input device to BlueZ path: prefer MAC (unique_name), fall back to name filter.
async fn find_bluez_path_for(
    conn: &Connection,
    dev: &Device,
    name_filter: &str,
) -> Option<OwnedObjectPath> {
    let proxy = ObjectManagerProxy::new(conn).await.ok()?;
    let objects = proxy.get_managed_objects().await.ok()?;
    let filter_lower = name_filter.to_lowercase();
    let mac = dev
        .unique_name()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_uppercase());

    for (path, interfaces) in objects {
        let Some(props) = interfaces.get("org.bluez.Device1") else { continue };
        if let Some(ref mac) = mac {
            if let Some(addr) = str_prop(props, "Address") {
                if addr.to_uppercase() == *mac {
                    return Some(path);
                }
            }
        } else if let Some(name) = str_prop(props, "Name") {
            if name.to_lowercase().contains(&filter_lower) {
                return Some(path);
            }
        }
    }
    None
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// Returns (center, threshold) per axis code. Threshold = 10% of axis range.
fn abs_thresholds(device: &Device) -> HashMap<u16, (i32, i32)> {
    let mut map = HashMap::new();
    if let Some(axes) = device.supported_absolute_axes() {
        if let Ok(state) = device.get_abs_state() {
            for axis in axes.iter() {
                if let Some(info) = state.get(axis.0 as usize) {
                    let min = info.minimum;
                    let max = info.maximum;
                    if max > min {
                        let center = (min + max) / 2;
                        let threshold = (max - min) / 10;
                        map.insert(axis.0, (center, threshold));
                    }
                }
            }
        }
    }
    map
}

async fn monitor_and_disconnect(
    label: String,
    input_dev: Device,
    conn: Connection,
    bluez_path: OwnedObjectPath,
    timeout_dur: Duration,
    debug: bool,
) {
    let builder = match BlueZDeviceProxy::builder(&conn).path(&bluez_path) {
        Ok(b) => b,
        Err(e) => { eprintln!("[{label}] invalid BlueZ path: {e}"); return; }
    };
    let device_proxy = match builder.build().await {
        Ok(p) => p,
        Err(e) => { eprintln!("[{label}] proxy error: {e}"); return; }
    };

    let thresholds = abs_thresholds(&input_dev);

    let (tx, mut rx) = mpsc::channel::<()>(32);
    let mut stream = match input_dev.into_event_stream() {
        Ok(s) => s,
        Err(e) => { eprintln!("[{label}] event stream: {e}"); return; }
    };

    let last_activity = Arc::new(AtomicU64::new(now_secs()));
    let last_event_name: Arc<Mutex<String>> = Arc::new(Mutex::new("(none)".into()));

    let debug_handle = if debug {
        let last = Arc::clone(&last_activity);
        let last_name = Arc::clone(&last_event_name);
        let idle = timeout_dur.as_secs();
        let lbl = label.clone();
        Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let ago = now_secs().saturating_sub(last.load(Ordering::Relaxed));
                let name = last_name.lock().unwrap().clone();
                println!("[{lbl}] last activity: {ago}s ago  ({name})  (timeout: {idle}s)");
            }
        }))
    } else {
        None
    };

    let last = Arc::clone(&last_activity);
    let last_name = Arc::clone(&last_event_name);
    let reader_handle = tokio::spawn(async move {
        loop {
            match stream.next_event().await {
                Ok(ev) => {
                    let active = match ev.event_type() {
                        evdev::EventType::SYNCHRONIZATION => false,
                        evdev::EventType::ABSOLUTE => match thresholds.get(&ev.code()) {
                            Some(&(center, threshold)) => {
                                (ev.value() - center).abs() > threshold
                            }
                            None => true,
                        },
                        _ => true,
                    };
                    if active {
                        *last_name.lock().unwrap() = format!(
                            "{:?}({}) = {}",
                            ev.event_type(),
                            ev.code(),
                            ev.value()
                        );
                        last.store(now_secs(), Ordering::Relaxed);
                        if tx.send(()).await.is_err() {
                            break;
                        }
                    }
                }
                Err(_) => break,
            }
        }
    });

    loop {
        match tokio::time::timeout(timeout_dur, rx.recv()).await {
            Ok(Some(())) => {}
            Ok(None) => {
                println!("[{label}] disconnected externally.");
                break;
            }
            Err(_) => {
                println!("[{label}] idle for {}s — disconnecting...", timeout_dur.as_secs());
                match device_proxy.disconnect().await {
                    Ok(()) => println!("[{label}] disconnected."),
                    Err(e) => eprintln!("[{label}] disconnect failed: {e}"),
                }
                break;
            }
        }
    }

    reader_handle.abort();
    if let Some(h) = debug_handle {
        h.abort();
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    let timeout_dur = Duration::from_secs(args.timeout);

    let conn = Connection::system().await.context("connect to D-Bus system bus")?;

    println!(
        "Watching for \"{}\" — idle timeout: {}s",
        args.name, args.timeout
    );

    // device_id -> running monitor task
    let mut tasks: HashMap<String, JoinHandle<()>> = HashMap::new();

    loop {
        // Drop handles for finished tasks.
        tasks.retain(|_, h| !h.is_finished());

        for dev in find_all_input_devices(&args.name) {
            let id = device_id(&dev);
            if tasks.contains_key(&id) {
                continue;
            }

            let Some(bluez_path) = find_bluez_path_for(&conn, &dev, &args.name).await else {
                continue;
            };

            let label = format!(
                "{} [{}]",
                dev.name().unwrap_or("unknown"),
                id
            );
            println!("Found: {label} → {bluez_path}");

            let handle = tokio::spawn(monitor_and_disconnect(
                label,
                dev,
                conn.clone(),
                bluez_path,
                timeout_dur,
                args.debug,
            ));
            tasks.insert(id, handle);
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
