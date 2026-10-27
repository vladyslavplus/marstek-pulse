# Marstek Pulse

A lightweight desktop utility for monitoring Marstek energy storage devices through their local, read-only OpenAPI. Built with **Rust**, **Slint**, and **Serde**, with UDP communication and no embedded browser or application cloud backend.

The initial device target is **Venus E 3.0**. Windows is the primary development and testing platform; Linux and macOS execution has not been verified.

Currently available: a native window and tray menu, English/Ukrainian interface text, and command-line device discovery and probing. Live readings in the window and remaining-runtime estimates are planned.

## Requirements

These instructions target Windows x64:

- [Git](https://git-scm.com/downloads) to clone the repository.
- [Rust via rustup](https://rust-lang.org/tools/install/), stable **1.92 or newer**, using the `x86_64-pc-windows-msvc` toolchain. Cargo is included.
- [Visual Studio C++ Build Tools](https://learn.microsoft.com/en-us/windows/dev-environment/rust/setup) with the **Desktop development with C++** workload and a Windows 10 or 11 SDK.

VS Code is optional. Recommended extensions: [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer) and [Slint](https://marketplace.visualstudio.com/items?itemName=Slint.slint). Slint itself is built by Cargo; no separate SDK installation is needed.

## Build and run

Open PowerShell. Replace `REPOSITORY_URL` with this repository's Git clone URL:

```powershell
git clone "REPOSITORY_URL" marstek-pulse
cd marstek-pulse
cargo run --locked --release
```

Run Cargo commands from the repository root: the folder containing `Cargo.toml`. For an existing checkout, open that folder in your terminal and run the last command. The first build downloads dependencies and compiles the application.

After building, you can launch the executable directly:

```powershell
.\target\release\marstek-pulse.exe
```

Use the language buttons to switch between Ukrainian and English. The tray menu provides **Show**, **Hide**, and **Exit**. The window also has an **Exit** button.

Until a tray action confirms availability, closing the window keeps it visible. `--start-hidden` also falls back to a visible window when tray availability is unknown. Use **Exit** to fully quit.

## Connect to a station

1. Set up the station in the official Marstek mobile app.
2. Connect the computer and station to the same reachable local network.
3. Enable **Local API** in the station settings and note its UDP port; the default is `30000`.

The vendor's [API documentation](https://static-eu.marstekcloud.com/ems/resource/agreement/MarstekDeviceOpenApi.pdf) describes activation requirements and possible effects on built-in station features. Marstek Pulse sends read-only requests.

From the repository root, run bounded discovery:

```powershell
cargo run --locked --release -- --discover --port 30000
```

To sample a specific station, obtain its IPv4 address from your router or device app, then replace `STATION_IPV4` below. Use the port configured in the mobile app:

```powershell
cargo run --locked --release -- --probe --host "STATION_IPV4" --port 30000
```

The probe requests device identity and three battery/system sample cycles, spaced 30 seconds apart. Output contains request outcomes and field availability; these commands do not populate the window with live readings. A timeout does not establish its cause.

---

This is an independent open-source project and is not affiliated with or endorsed by Marstek.
