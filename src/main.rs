//! Cratebox for Linux: two-deck DJ software for the Pioneer DDJ-FLX4 and friends.
use eframe::egui;

fn icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../assets/icon-512.png");
    let dec = png::Decoder::new(std::io::Cursor::new(&bytes[..]));
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).ok()?;
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()].chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        _ => return None,
    };
    Some(egui::IconData { rgba, width: info.width, height: info.height })
}

#[cfg(target_arch = "x86_64")]
#[inline(never)]
fn cpu_ok() -> bool {
    use std::arch::x86_64::{__cpuid, __cpuid_count};
    let max = unsafe { __cpuid(0) }.eax;
    if max < 7 { return false; }
    let l1 = unsafe { __cpuid(1) };
    let l7 = unsafe { __cpuid_count(7, 0) };
    let fma = l1.ecx & (1 << 12) != 0;
    let avx2 = l7.ebx & (1 << 5) != 0;
    let bmi2 = l7.ebx & (1 << 8) != 0;
    std::hint::black_box(fma && avx2 && bmi2)
}

fn uid() -> Option<u32> {
    let st = std::fs::read_to_string("/proc/self/status").ok()?;
    st.lines().find(|l| l.starts_with("Uid:"))?.split_whitespace().nth(2)?.parse().ok()
}

/// Find the desktop's display when the environment doesn't say (terminals that drop variables,
/// launchers, `su`): a Wayland socket in the runtime dir, or the X authority file the login made.
fn fix_display_env() {
    let uid = uid().unwrap_or(1000);
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| format!("/run/user/{uid}"));
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() && std::path::Path::new(&runtime).is_dir() { std::env::set_var("XDG_RUNTIME_DIR", &runtime); }
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        for n in ["wayland-0", "wayland-1"] {
            if std::path::Path::new(&runtime).join(n).exists() { std::env::set_var("WAYLAND_DISPLAY", n); break; }
        }
    }
    if std::env::var_os("DISPLAY").is_some() && std::env::var_os("XAUTHORITY").is_none() {
        let home_auth = dirs::home_dir().map(|h| h.join(".Xauthority"));
        if !home_auth.as_ref().is_some_and(|p| p.exists()) {
            // GNOME/Mutter, KDE, GDM, LightDM/SDDM keep it in the runtime dir under various names
            let found = std::fs::read_dir(&runtime).ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| {
                let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                n.starts_with(".mutter-Xwaylandauth") || n.starts_with("xauth_") || n == "Xauthority" || n.starts_with(".Xauthority")
            })).or_else(|| { let g = std::path::Path::new(&runtime).join("gdm/Xauthority"); g.exists().then_some(g) });
            if let Some(f) = found { std::env::set_var("XAUTHORITY", f); }
        }
    }
}

fn display_help(err: &dyn std::fmt::Display) {
    eprintln!("\nCratebox couldn't open a window: {err}\n");
    if uid() == Some(0) {
        eprintln!("You're running it as root (sudo?). Run it as yourself instead: just `cratebox`, no sudo.");
        eprintln!("(Root isn't allowed onto your desktop's display, and Cratebox needs no special rights.)");
    } else if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("There's no desktop in this terminal (an SSH session or a text console?).");
        eprintln!("Start Cratebox from the app menu, or from a terminal inside your desktop session.");
    } else {
        eprintln!("The display refused the connection. Things to try:");
        eprintln!("  - start it from the app menu (Cratebox) instead of this terminal");
        eprintln!("  - if you switched user with su/sudo in this terminal, run it as the logged-in user");
        eprintln!("  - on X11: `xhost +SI:localuser:$USER` once, then run it again");
        eprintln!("DISPLAY={:?} WAYLAND_DISPLAY={:?} XAUTHORITY={:?}", std::env::var("DISPLAY").ok(), std::env::var("WAYLAND_DISPLAY").ok(), std::env::var("XAUTHORITY").ok());
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    // built for x86-64-v3 (AVX2/FMA/BMI2): say so plainly on an older CPU instead of crashing.
    // (is_x86_feature_detected! would be folded to `true` at compile time here, so ask CPUID directly)
    #[cfg(target_arch = "x86_64")]
    if !cpu_ok() {
        use std::io::Write;
        let _ = std::io::stderr().write_all(b"Cratebox was built for x86-64-v3 CPUs (Intel Haswell 2013+ / AMD Zen 1+) and this CPU lacks AVX2/FMA/BMI2.\nBuild it for this machine: cargo build --release --config 'build.rustflags=[]'\n");
        std::process::exit(1);
    }
    if args.iter().any(|a| a == "--version" || a == "-V") { println!("cratebox {}", env!("CARGO_PKG_VERSION")); return Ok(()); }
    // --screenshot FILE [FRAMES]: render, save a PNG and quit (used by the tests)
    let shot = args.iter().position(|a| a == "--screenshot").and_then(|i| args.get(i + 1).map(|p| (std::path::PathBuf::from(p), args.get(i + 2).and_then(|n| n.parse().ok()).unwrap_or(90u64))));
    fix_display_env();
    let mut vp = egui::ViewportBuilder::default().with_title("Cratebox").with_app_id("cratebox").with_inner_size([1440.0, 900.0]).with_min_inner_size([1100.0, 720.0]);
    if let Some(i) = icon() { vp = vp.with_icon(std::sync::Arc::new(i)); }
    // --kiosk: full screen, no title bar (the Cratebox kiosk session on the distro ISO)
    if args.iter().any(|a| a == "--kiosk") { vp = vp.with_fullscreen(true).with_decorations(false); }
    let opts = eframe::NativeOptions { viewport: vp, vsync: true, ..Default::default() };
    let r = eframe::run_native("Cratebox", opts, Box::new(move |cc| {
        let mut g = cratebox::ui::Gui::new(cc);
        g.test_shot = shot.clone();
        Ok(Box::new(g))
    }));
    if let Err(e) = &r { display_help(e); std::process::exit(1); }
    r
}
