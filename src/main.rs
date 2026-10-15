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
    let mut vp = egui::ViewportBuilder::default().with_title("Cratebox").with_app_id("cratebox").with_inner_size([1440.0, 900.0]).with_min_inner_size([1100.0, 720.0]);
    if let Some(i) = icon() { vp = vp.with_icon(std::sync::Arc::new(i)); }
    let opts = eframe::NativeOptions { viewport: vp, vsync: true, ..Default::default() };
    eframe::run_native("Cratebox", opts, Box::new(move |cc| {
        let mut g = cratebox::ui::Gui::new(cc);
        g.test_shot = shot.clone();
        Ok(Box::new(g))
    }))
}
