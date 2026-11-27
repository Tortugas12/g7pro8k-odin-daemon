# GameSir G7 Pro 8K — AYN Odin 3 Pro Day-1 Testing & Troubleshooting Runbook

This step-by-step checklist walks you through installing, verifying, and troubleshooting `g7pro8k-odin-daemon` on your rooted **AYN Odin 3 Pro** (Snapdragon 8 Elite / `arm64`) when your **plain-black GameSir G7 Pro 8K PC** arrives.

> **Safety Guarantee:** This daemon **never** calls `EVIOCGRAB` or writes to your Odin 3 Pro's built-in controller, **only** claims allowlisted GameSir `0x3537` USB devices, **never** sends firmware/register write commands (standard 8-byte XInput rumble only), and binds its Companion Web App strictly to `127.0.0.1:8787`.

---

## 1. Installing & Launching on AYN Odin 3 Pro

### Option A (Easiest — Single Script Directly on Odin 3 Pro, Zero ADB or Terminal)
1. Download **`g7pro8k-odin3-oneclick.sh`** (2.2 MB) onto your Odin 3 Pro (for example, into `Downloads/`).
   - *(Optional: also download **`g7pro8k-odin3-stop.sh`** if you ever want to fully stop the daemon from Odin Settings instead of tapping **Pause Daemon** in the Web UI.)*
2. Open **Odin Settings → Run script as root** and select **`g7pro8k-odin3-oneclick.sh`**.
3. The script automatically:
   - Extracts the statically compiled `arm64` daemon + embedded Companion Web App to `/data/local/tmp/g7pro8k/` (with SHA-256 verification).
   - Writes the default **Xbox 360** `config.json` on first run (and preserves your saved settings on every future run).
   - Starts `g7pro8k-odin-daemon` cleanly detached in the background.
   - Opens **`http://127.0.0.1:8787`** automatically in your browser.
4. *(Optional)* Tap Chrome's menu (**⋮**) → **Add to Home screen** to pin the offline Companion Web App to your Odin 3 launcher.

### Option B (Build & Push via ADB from a PC)
```bash
./scripts/build-odin3-arm64.sh
```
This builds the static `aarch64-unknown-linux-musl` binary and stages both the individual files and the single-file `g7pro8k-odin3-oneclick.sh` / `g7pro8k-odin3-stop.sh` scripts in `dist/odin3-arm64/`.

```bash
adb shell 'su -c "mkdir -p /data/local/tmp/g7pro8k"'
adb push dist/odin3-arm64/. /data/local/tmp/g7pro8k/
adb shell 'su -c "chmod +x /data/local/tmp/g7pro8k/g7pro8k-odin-daemon /data/local/tmp/g7pro8k/g7pro8k-ctl.sh"'
adb shell 'su -c "/data/local/tmp/g7pro8k/g7pro8k-ctl.sh doctor"'
adb shell 'su -c "/data/local/tmp/g7pro8k/g7pro8k-ctl.sh start"'
adb shell 'su -c "/data/local/tmp/g7pro8k/g7pro8k-ctl.sh status"'
```

### Open & install the Companion Web App on Odin 3 Pro
1. If it did not auto-open, launch **Chrome** on your AYN Odin 3 Pro and navigate to:
   **`http://127.0.0.1:8787`**
2. Tap Chrome's menu (**⋮**) → **Add to Home screen** (or **Install app**) to pin the offline PWA directly to your Odin 3 launcher in landscape mode.

---

## 2. Zero-Interference Check (Before Plugging In the GameSir)

Before attaching the GameSir G7 Pro 8K dongle or USB-C cable, verify that your built-in Odin 3 controls remain 100% untouched:

1. In the Companion Web App (`http://127.0.0.1:8787`), check the top status bar:
   - Connection pill reads **`Disconnected`** (amber).
   - Built-in badge reads **`Built-in Odin 3: ... (Untouched)`** (cyan).
2. Confirm the blue info banner appears:
   > *"GameSir G7 Pro 8K Disconnected: Plug in the 2.4GHz USB dongle or USB-C cable. Showing live read-only AYN Odin 3 built-in controller below (100% isolated)."*
3. Confirm the left panel automatically switched to the **AYN Odin 3 Pro (Read-Only • Isolated)** handheld SVG.
4. Move your Odin 3 built-in sticks, squeeze `L2`/`R2`, and press `A/B/X/Y`, D-pad, `L1`/`R1`, `Select`/`Start`:
   - Every button lights up in cyan on the Odin 3 SVG.
   - Live analog percentages (`L2`/`R2` `0%–100%`, `LS`/`RS` `-100%` to `+100%` + radial magnitude `%`) and the dual circularity plots update smoothly.
5. Switch briefly to Android's home screen or any game and verify the built-in Odin 3 controller behaves identically to before the daemon started.

---

## 3. First GameSir G7 Pro 8K Plug-In & Day-1 Capture

Plug in either the **2.4GHz USB dongle** (recommended first) or connect via **USB-C cable**.

### Confirm connection & run USB Probe
1. In the top bar, confirm the status pill turns emerald green:
   - **`GameSir G7 Pro 8K • 2.4GHz Dongle`** (`3537:1035`), or
   - **`GameSir G7 Pro 8K • USB-C Wired`** (`3537:1034`).
2. Confirm the `USB Hz → Emit Hz` readout shows active polling (e.g. `1000–8000 Hz → 1000 Hz`).
3. Expand the **Day-1 Hardware Diagnostics** drawer at the bottom of the page and tap **Run USB Probe**:
   - Confirm VID `0x3537` and PID `0x1034` (wired) or `0x1035` (dongle) appear with status `supported` and Interface 0 (`0xff/0x5d/0x01`) + Interface 1 (`0x03/...`).

#### Troubleshooting Wired Fallback (`3537:10b8`)
If you plug in via USB-C cable and see the amber **`10b8 Fallback Warning`** banner:
- Linux dropped wired `3537:1034` out of XInput mode into `3537:10b8`.
- **Instant fix:** Hold **`View + Menu`** on the GameSir G7 Pro 8K for ~3 seconds until it re-enumerates as `3537:1034` (or use the 2.4GHz USB dongle `3537:1035`).
- *(Linux kernel note: on kernels with configurable boot args, adding `usbcore.quirks=3537:1034:r` prevents the drop.)*

### Record a 10-Second Day-1 Capture (Verifying `L4/R4/L5/R5` Byte Offsets)
1. Inside the **Day-1 Hardware Diagnostics** drawer, tap **Record 10s Capture (.jsonl)**.
2. During the 10-second progress bar, deliberately press:
   - `A`, `B`, `X`, `Y`, D-pad, `LB`, `RB`, `View`, `Menu`, `LS`, `RS`
   - Full sweep of Left Stick, Right Stick, `LT`, and `RT`
   - Each extra button one by one: **`L4` → `R4` → `L5` → `R5`**
3. Chrome downloads `g7pro8k-capture.jsonl`.
4. **If `L4/R4/L5/R5` did not light up on the SVG** (e.g. due to a newer firmware revision shifting report `0x12` byte offsets) or if your edition reports a new `0x3537` PID:
   - Inspect the `hid12` lines in `g7pro8k-capture.jsonl` to see which byte/bit toggles when pressing `L4/R4/L5/R5`.
   - Edit `/data/local/tmp/g7pro8k/config.json` (`hid12_layout` or `extra_pids`) — **no recompile required**.
   - Test offline on your PC or Odin 3 with:
     ```bash
     g7pro8k-odin-daemon validate-config --config /data/local/tmp/g7pro8k/config.json
     g7pro8k-odin-daemon replay --config /data/local/tmp/g7pro8k/config.json --input g7pro8k-capture.jsonl --dry-run
     ```

---

## 4. Testing Xbox 360 vs. DualShock 4 Profiles, Circularity, Rumble & Extra Buttons

1. **Test Xbox 360 Profile (Default):**
   - Press `A/B/X/Y`, bumpers, triggers, sticks, and `L4/R4/L5/R5` and watch the **Xbox 360 SVG** highlight every button in emerald green with live analog `%`.
   - Check **`Test Circularity`**, rotate both sticks smoothly around their outer rings, check the live average circularity error `%`, then tap **Reset Trace**.
   - Tap **Test Vibration** and verify a crisp 300ms dual-motor rumble pulse.
2. **Test Extra-Button Presets:**
   - Try tapping **Gamepad Mirrors (`L3/R3/LB/RB`)**, then switch back to **PC Keys (`F1–F4`)** or customize individual slots (`Keyboard Key`, `Gamepad Button`, or `Disabled`).
   - Verify the amber **`Unsaved changes`** badge appears in the sticky save bar.
3. **Test DualShock 4 Profile Switch & Hot-Reload:**
   - Tap **DualShock 4 (`054C:09CC`)** in the right column.
   - Watch the left panel immediately preview the cobalt-blue **DualShock 4 SVG** with PlayStation symbols (`✕`, `○`, `□`, `△`).
   - Tap **Save & Apply** — confirm the toast *"Saved config.json & hot-reloaded daemon!"* appears and the daemon cleanly recreates the virtual `uinput` controller without restarting.
   - Switch back to **Xbox 360 (`045E:028E`)** and tap **Save & Apply** when done so games show `A/B/X/Y` icons by default.
4. **Test Pause / Resume:**
   - Tap **Pause Daemon** in the top bar → verify USB + virtual devices are released cleanly and the pill changes to `Paused`.
   - Tap **Resume Daemon** → verify the controller reconnects automatically within ~1 second.

---

## 5. Verifying in Real Apps & Games on Odin 3 Pro

### GameNative, DroidDeck, Steam Link, Emulators & Native Android Games (Xbox 360 Profile)
1. Ensure **Xbox 360 (`045E:028E`)** is active in the Web UI (or run `g7pro8k-ctl.sh profile xbox360`).
2. Launch **GameNative**, **DroidDeck**, **Steam Link**, or a native Android game:
   - Confirm in-game prompts show **A / B / X / Y** icons matching the physical controller.
   - Confirm smooth stick response at `1000 Hz` emit cap (try `500 Hz` in the Web UI if any specific Android app exhibits input queue jitter).
   - Bind **`L4/R4/L5/R5`** (emitting `KEY_F1`–`KEY_F4` from the companion virtual keyboard) inside emulator hotkeys (save state, fast-forward, menu) or PC streaming overlays.

### PS5 Remote Play (Opt-In DualShock 4 Profile)
1. In the Web UI, select **DualShock 4 (`054C:09CC`)** and tap **Save & Apply** (or run `g7pro8k-ctl.sh profile ds4`).
2. Open **PS Remote Play** (or PXPlay/Chiaki) and confirm `"Wireless Controller"` is recognized with working sticks, `L2`/`R2`, PlayStation buttons, and rumble.
3. When finished with PS5 Remote Play, tap **Xbox 360** → **Save & Apply** to restore `A/B/X/Y` prompts everywhere else.

---

## 6. Quick 30-Second Hardware Fallback Check (Built-in 1000Hz DS4 Mode)

As an independent hardware sanity check (for example, if you ever want to isolate an Android app issue from the daemon):
1. Tap **Pause Daemon** in the Web UI (or run `g7pro8k-ctl.sh stop`).
2. Switch the GameSir G7 Pro 8K hardware mode to its built-in **1000Hz DS4 mode** per the GameSir manual and confirm Android detects it natively.
3. Switch the controller back to **PC / XInput mode** and tap **Resume Daemon** (or `g7pro8k-ctl.sh start`) to restore full Xbox 360 A/B/X/Y + independent `L4/R4/L5/R5` + Web UI functionality.

---

## Quick On-Device Command Reference

```bash
# Start / stop / check status
su -c "/data/local/tmp/g7pro8k/g7pro8k-ctl.sh start"
su -c "/data/local/tmp/g7pro8k/g7pro8k-ctl.sh stop"
su -c "/data/local/tmp/g7pro8k/g7pro8k-ctl.sh status"

# Switch profiles from shell
su -c "/data/local/tmp/g7pro8k/g7pro8k-ctl.sh profile xbox360"
su -c "/data/local/tmp/g7pro8k/g7pro8k-ctl.sh profile ds4"

# Diagnostics & logs
su -c "/data/local/tmp/g7pro8k/g7pro8k-ctl.sh probe"
su -c "/data/local/tmp/g7pro8k/g7pro8k-ctl.sh doctor"
su -c "/data/local/tmp/g7pro8k/g7pro8k-ctl.sh logs"
```
