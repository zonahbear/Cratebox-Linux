//! In-place radix-2 FFT (the same algorithm the web version uses), plus a cached plan for repeated sizes.

pub struct Fft {
    n: usize,
    cos: Vec<f64>,
    sin: Vec<f64>,
    rev: Vec<u32>,
}

impl Fft {
    pub fn new(n: usize) -> Fft {
        assert!(n.is_power_of_two() && n >= 2);
        let bits = n.trailing_zeros();
        let rev = (0..n as u32).map(|i| i.reverse_bits() >> (32 - bits)).collect();
        let half = n / 2;
        let cos = (0..half).map(|k| (-2.0 * std::f64::consts::PI * k as f64 / n as f64).cos()).collect();
        let sin = (0..half).map(|k| (-2.0 * std::f64::consts::PI * k as f64 / n as f64).sin()).collect();
        Fft { n, cos, sin, rev }
    }
    pub fn len(&self) -> usize { self.n }
    pub fn is_empty(&self) -> bool { self.n == 0 }

    /// forward transform of (re, im), in place
    pub fn run(&self, re: &mut [f64], im: &mut [f64]) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i] as usize;
            if i < j { re.swap(i, j); im.swap(i, j); }
        }
        let mut len = 2;
        while len <= n {
            let h = len / 2;
            let step = n / len;
            let mut i = 0;
            while i < n {
                for k in 0..h {
                    let (wr, wi) = (self.cos[k * step], self.sin[k * step]);
                    let a = i + k;
                    let b = a + h;
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr; im[b] = im[a] - ti;
                    re[a] += tr; im[a] += ti;
                }
                i += len;
            }
            len <<= 1;
        }
    }
}

pub fn hann(n: usize) -> Vec<f64> {
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sine_lands_in_its_bin() {
        let n = 1024;
        let f = Fft::new(n);
        let mut re: Vec<f64> = (0..n).map(|i| (2.0 * std::f64::consts::PI * 50.0 * i as f64 / n as f64).sin()).collect();
        let mut im = vec![0.0; n];
        f.run(&mut re, &mut im);
        let mag: Vec<f64> = (0..n / 2).map(|k| re[k].hypot(im[k])).collect();
        let best = (0..n / 2).max_by(|a, b| mag[*a].partial_cmp(&mag[*b]).unwrap()).unwrap();
        assert_eq!(best, 50);
        assert!((mag[50] - n as f64 / 2.0).abs() < 1e-6);
    }
}
