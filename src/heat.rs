//! HEAT: TrueCurve heat curves for a Puffco Peak Pro / Proxy (UI thread). Placeholder while the port lands.

#[derive(Default)]
pub struct Heat {}

impl Heat {
    pub fn new() -> Heat { Heat::default() }
    pub fn label(&self) -> String { "HEAT".into() }
    pub fn lit(&self) -> bool { false }
}
