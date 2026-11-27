use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nusb::descriptors::TransferType;
use nusb::transfer::{Buffer, Direction, In, Interrupt, Out};
use nusb::{Endpoint, Interface, MaybeFuture};
use tracing::{debug, info, warn};

use crate::device_registry::{classify_usb_device, DeviceMatch, FallbackQuirkInfo, MatchedDevice};
use crate::diagnostics::{ProbeDeviceEntry, ProbeEndpointEntry, ProbeInterfaceEntry, ProbeReport};
use crate::protocol::state::RumbleState;
use crate::protocol::xinput::encode_rumble_report;
use crate::transport::{UsbPacket, UsbTransport, UsbTransportError};

const XINPUT_IFACE_CLASS: u8 = 0xFF;
const XINPUT_IFACE_SUBCLASS: u8 = 0x5D;
const XINPUT_IFACE_PROTOCOL: u8 = 0x01;
const HID_IFACE_CLASS: u8 = 0x03;

const IN_FLIGHT_URBS: usize = 4;
const PACKET_CHANNEL_BOUND: usize = 512;

#[derive(Debug, Clone, Copy)]
struct EndpointLayout {
    xinput_iface: u8,
    xinput_in_ep: u8,
    xinput_in_mps: usize,
    xinput_out_ep: Option<u8>,
    hid12_iface: Option<u8>,
    hid12_in_ep: Option<u8>,
    hid12_in_mps: usize,
}

/// Pure-Rust `nusb` USB transport for GameSir G7 Pro 8K.
///
/// Safety invariants:
/// - Strictly filters enumerated devices via `device_registry::classify_usb_device`.
/// - Keeps 4 interrupt IN URBs in flight on Interface 0 (and optional Interface 1) so
///   8kHz polling never stalls the dongle.
/// - Only writes standard 8-byte XInput rumble reports on Interface 0 OUT.
pub struct NusbTransport {
    matched: MatchedDevice,
    has_hid12: bool,
    rx: mpsc::Receiver<Result<UsbPacket, String>>,
    xinput_out: Option<Endpoint<Interrupt, Out>>,
    stop: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
    _ifaces: Vec<Interface>,
}

impl NusbTransport {
    /// Enumerates USB devices and opens the first allowlisted GameSir G7 Pro 8K device.
    pub fn open_first(extra_pids: &[u16]) -> Result<Self, UsbTransportError> {
        let devices = nusb::list_devices()
            .wait()
            .map_err(|e| UsbTransportError::Usb(format!("USB enumeration failed: {e}")))?;

        let mut fallback_quirk: Option<FallbackQuirkInfo> = None;

        for info in devices {
            let vid = info.vendor_id();
            let pid = info.product_id();
            match classify_usb_device(vid, pid, extra_pids) {
                DeviceMatch::Supported(matched) => {
                    return Self::open_matched(info, matched);
                }
                DeviceMatch::XInputFallbackQuirk(quirk) => {
                    fallback_quirk = Some(quirk);
                }
                DeviceMatch::Ignored => {}
            }
        }

        if let Some(quirk) = fallback_quirk {
            return Err(UsbTransportError::FallbackQuirk(quirk));
        }

        Err(UsbTransportError::NotFound)
    }

    fn open_matched(
        info: nusb::DeviceInfo,
        matched: MatchedDevice,
    ) -> Result<Self, UsbTransportError> {
        let device = info.open().wait().map_err(|e| {
            UsbTransportError::Usb(format!(
                "failed to open {:04x}:{:04x}: {e}",
                matched.vid, matched.pid
            ))
        })?;

        let layout = discover_endpoints(&device)?;
        info!(
            vid = format_args!("{:04x}", matched.vid),
            pid = format_args!("{:04x}", matched.pid),
            name = %matched.name,
            xinput_if = layout.xinput_iface,
            xinput_in = format_args!("0x{:02x}", layout.xinput_in_ep),
            xinput_out = ?layout.xinput_out_ep.map(|e| format!("0x{e:02x}")),
            hid12_if = ?layout.hid12_iface,
            hid12_in = ?layout.hid12_in_ep.map(|e| format!("0x{e:02x}")),
            "claimed GameSir G7 Pro 8K USB transport"
        );

        let iface0 = device
            .detach_and_claim_interface(layout.xinput_iface)
            .wait()
            .map_err(|e| {
                UsbTransportError::Usb(format!(
                    "failed to claim XInput interface {}: {e}",
                    layout.xinput_iface
                ))
            })?;

        let xinput_in_ep = iface0
            .endpoint::<Interrupt, In>(layout.xinput_in_ep)
            .map_err(|e| UsbTransportError::Usb(format!("XInput IN endpoint error: {e}")))?;

        let xinput_out = match layout.xinput_out_ep {
            Some(addr) => match iface0.endpoint::<Interrupt, Out>(addr) {
                Ok(ep) => Some(ep),
                Err(err) => {
                    warn!(error = %err, "XInput OUT rumble endpoint unavailable; continuing without rumble");
                    None
                }
            },
            None => None,
        };

        let mut claimed_ifaces = vec![iface0];
        let mut hid12_in_ep_opt = None;

        if let (Some(if1_num), Some(ep1_addr)) = (layout.hid12_iface, layout.hid12_in_ep) {
            match device.detach_and_claim_interface(if1_num).wait() {
                Ok(iface1) => match iface1.endpoint::<Interrupt, In>(ep1_addr) {
                    Ok(ep) => {
                        hid12_in_ep_opt = Some((ep, layout.hid12_in_mps));
                        claimed_ifaces.push(iface1);
                    }
                    Err(err) => {
                        warn!(error = %err, "HID 0x12 IN endpoint unavailable; degrading gracefully to XInput-only");
                    }
                },
                Err(err) => {
                    warn!(iface = if1_num, error = %err, "could not claim Interface 1; degrading gracefully to XInput-only");
                }
            }
        } else {
            debug!("Interface 1 vendor HID channel not present; running in XInput-only mode");
        }

        let has_hid12 = hid12_in_ep_opt.is_some();
        let (tx, rx) = mpsc::sync_channel::<Result<UsbPacket, String>>(PACKET_CHANNEL_BOUND);
        let stop = Arc::new(AtomicBool::new(false));
        let epoch = Instant::now();
        let mut workers = Vec::new();

        workers.push(spawn_interrupt_reader(
            "g7pro-xinput-in",
            xinput_in_ep,
            layout.xinput_in_mps.max(64),
            true,
            epoch,
            Arc::clone(&stop),
            tx.clone(),
        ));

        if let Some((hid_ep, hid_mps)) = hid12_in_ep_opt {
            workers.push(spawn_interrupt_reader(
                "g7pro-hid12-in",
                hid_ep,
                hid_mps.max(64),
                false,
                epoch,
                Arc::clone(&stop),
                tx,
            ));
        }

        Ok(Self {
            matched,
            has_hid12,
            rx,
            xinput_out,
            stop,
            workers,
            _ifaces: claimed_ifaces,
        })
    }
}

impl UsbTransport for NusbTransport {
    fn device(&self) -> &MatchedDevice {
        &self.matched
    }

    fn has_hid12(&self) -> bool {
        self.has_hid12
    }

    fn recv_packet(&mut self, timeout: Duration) -> Result<Option<UsbPacket>, UsbTransportError> {
        match self.rx.recv_timeout(timeout) {
            Ok(Ok(pkt)) => Ok(Some(pkt)),
            Ok(Err(err)) => Err(UsbTransportError::Disconnected(err)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(UsbTransportError::Disconnected(
                "USB reader worker terminated".to_string(),
            )),
        }
    }

    fn send_rumble(&mut self, rumble: RumbleState) -> Result<(), UsbTransportError> {
        let Some(ep) = self.xinput_out.as_mut() else {
            return Ok(());
        };
        let payload = encode_rumble_report(rumble);
        let buf = Buffer::from(payload.to_vec());
        let completion = ep.transfer_blocking(buf, Duration::from_millis(50));
        if let Err(err) = completion.status {
            debug!(error = %err, "rumble OUT transfer warning");
        }
        Ok(())
    }
}

impl Drop for NusbTransport {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
    }
}

fn spawn_interrupt_reader(
    thread_name: &'static str,
    mut ep: Endpoint<Interrupt, In>,
    packet_size: usize,
    is_xinput: bool,
    epoch: Instant,
    stop: Arc<AtomicBool>,
    tx: mpsc::SyncSender<Result<UsbPacket, String>>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name(thread_name.to_string())
        .spawn(move || {
            for _ in 0..IN_FLIGHT_URBS {
                ep.submit(ep.allocate(packet_size));
            }

            while !stop.load(Ordering::Relaxed) {
                let Some(completion) = ep.wait_next_complete(Duration::from_millis(50)) else {
                    continue;
                };

                match completion.status {
                    Ok(()) => {
                        let ts_us = epoch.elapsed().as_micros() as u64;
                        let data = completion.buffer[..completion.actual_len].to_vec();
                        let pkt = if is_xinput {
                            UsbPacket::XInputIn {
                                timestamp_us: ts_us,
                                data,
                            }
                        } else {
                            UsbPacket::Hid12In {
                                timestamp_us: ts_us,
                                data,
                            }
                        };

                        // Non-blocking send so the USB reader NEVER stalls the 8kHz dongle.
                        let _ = tx.try_send(Ok(pkt));
                        ep.submit(completion.buffer);
                    }
                    Err(err) => {
                        if !stop.load(Ordering::Relaxed) {
                            if is_xinput {
                                let _ = tx.try_send(Err(format!("XInput USB IN error: {err}")));
                                break;
                            }
                            debug!(error = %err, "HID 0x12 reader error; stopping optional HID channel");
                            break;
                        }
                    }
                }
            }

            ep.cancel_all();
        })
        .expect("failed to spawn USB reader thread")
}

fn discover_endpoints(device: &nusb::Device) -> Result<EndpointLayout, UsbTransportError> {
    let config = device
        .active_configuration()
        .map_err(|e| UsbTransportError::Usb(format!("failed to read USB configuration: {e}")))?;

    let mut xinput_iface: Option<u8> = None;
    let mut xinput_in: Option<(u8, usize)> = None;
    let mut xinput_out: Option<u8> = None;

    let mut hid12_iface: Option<u8> = None;
    let mut hid12_in: Option<(u8, usize)> = None;

    for iface in config.interface_alt_settings() {
        let num = iface.interface_number();
        let is_xinput = iface.class() == XINPUT_IFACE_CLASS
            && iface.subclass() == XINPUT_IFACE_SUBCLASS
            && iface.protocol() == XINPUT_IFACE_PROTOCOL;

        if is_xinput || (xinput_iface.is_none() && num == 0) {
            let mut in_ep = None;
            let mut out_ep = None;
            for ep in iface.endpoints() {
                if ep.transfer_type() != TransferType::Interrupt {
                    continue;
                }
                match ep.direction() {
                    Direction::In if in_ep.is_none() => {
                        in_ep = Some((ep.address(), ep.max_packet_size()));
                    }
                    Direction::Out if out_ep.is_none() => {
                        out_ep = Some(ep.address());
                    }
                    _ => {}
                }
            }
            if let Some(found_in) = in_ep {
                xinput_iface = Some(num);
                xinput_in = Some(found_in);
                xinput_out = out_ep;
            }
        } else if (iface.class() == HID_IFACE_CLASS || num == 1) && hid12_iface.is_none() {
            for ep in iface.endpoints() {
                if ep.transfer_type() == TransferType::Interrupt && ep.direction() == Direction::In
                {
                    hid12_iface = Some(num);
                    hid12_in = Some((ep.address(), ep.max_packet_size()));
                    break;
                }
            }
        }
    }

    let (Some(xinput_iface), Some((xinput_in_ep, xinput_in_mps))) = (xinput_iface, xinput_in)
    else {
        return Err(UsbTransportError::Usb(
            "could not locate XInput interrupt IN endpoint on Interface 0".to_string(),
        ));
    };

    Ok(EndpointLayout {
        xinput_iface,
        xinput_in_ep,
        xinput_in_mps,
        xinput_out_ep: xinput_out,
        hid12_iface,
        hid12_in_ep: hid12_in.map(|(a, _)| a),
        hid12_in_mps: hid12_in.map_or(64, |(_, m)| m),
    })
}

/// Safe read-only USB probe used by CLI `probe` and Web UI `GET /api/probe`.
///
/// Safety invariant: enumerates sysfs metadata for overview, and ONLY opens allowlisted
/// `0x3537` GameSir devices to inspect interface/endpoint descriptors.
pub fn probe_usb_devices(extra_pids: &[u16]) -> Result<ProbeReport, UsbTransportError> {
    if !std::path::Path::new("/sys/bus/usb/devices").exists() {
        return Ok(ProbeReport {
            gamesir_count: 0,
            quirk_warning: None,
            devices: Vec::new(),
        });
    }

    let devices = nusb::list_devices()
        .wait()
        .map_err(|e| UsbTransportError::Usb(format!("USB enumeration failed: {e}")))?;

    let mut entries = Vec::new();
    let mut quirk_warning = None;

    for info in devices {
        let vid = info.vendor_id();
        let pid = info.product_id();
        let classification = classify_usb_device(vid, pid, extra_pids);

        if let DeviceMatch::XInputFallbackQuirk(ref quirk) = classification {
            quirk_warning = Some(quirk.clone());
        }

        let mut interfaces = Vec::new();
        // Safety invariant: ONLY open allowlisted GameSir devices to read active descriptors.
        if classification.is_supported() {
            if let Ok(dev) = info.open().wait() {
                if let Ok(cfg) = dev.active_configuration() {
                    for iface in cfg.interface_alt_settings() {
                        let endpoints = iface
                            .endpoints()
                            .map(|ep| ProbeEndpointEntry {
                                address: format!("0x{:02x}", ep.address()),
                                direction: match ep.direction() {
                                    Direction::In => "in".to_string(),
                                    Direction::Out => "out".to_string(),
                                },
                                transfer_type: format!("{:?}", ep.transfer_type()),
                                max_packet_size: ep.max_packet_size(),
                                interval: ep.interval(),
                            })
                            .collect();
                        interfaces.push(ProbeInterfaceEntry {
                            interface_number: iface.interface_number(),
                            class: format!("0x{:02x}", iface.class()),
                            subclass: format!("0x{:02x}", iface.subclass()),
                            protocol: format!("0x{:02x}", iface.protocol()),
                            endpoints,
                        });
                    }
                }
            }
        }

        entries.push(ProbeDeviceEntry {
            bus_id: info.bus_id().to_string(),
            device_address: info.device_address(),
            vid: format!("0x{vid:04x}"),
            pid: format!("0x{pid:04x}"),
            manufacturer: info.manufacturer_string().map(ToString::to_string),
            product: info.product_string().map(ToString::to_string),
            classification,
            interfaces,
        });
    }

    Ok(ProbeReport {
        gamesir_count: entries
            .iter()
            .filter(|e| e.classification.is_supported())
            .count(),
        quirk_warning,
        devices: entries,
    })
}
